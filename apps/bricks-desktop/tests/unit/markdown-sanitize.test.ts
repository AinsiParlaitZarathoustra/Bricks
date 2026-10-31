// @vitest-environment happy-dom
// Sanitizing profiles of the assistant-message pipeline, KaTeX rendering with the
// real library, and the bounds of Mermaid rendering. Mermaid's own layout needs a
// real browser: here only its failure path is checked (a clean error, no throw).
import { describe, expect, it } from 'vitest';
import katexLib from 'katex';

// happy-dom 20.14 answers '' to `Node.prototype.nodeName` (elements answer correctly
// on their own class). DOMPurify 3.4 reads tag names through that prototype getter,
// so every element looked forbidden and its removal stopped the walk. This shim gives
// the standard answer for each node type, in this test file only: browsers already do.
const nodeNameFallback = Object.getOwnPropertyDescriptor(Node.prototype, 'nodeName');
Object.defineProperty(Node.prototype, 'nodeName', {
	configurable: true,
	get(this: Node) {
		switch (this.nodeType) {
			case Node.ELEMENT_NODE:
				return (this as Element).tagName;
			case Node.TEXT_NODE:
				return '#text';
			case Node.CDATA_SECTION_NODE:
				return '#cdata-section';
			case Node.PROCESSING_INSTRUCTION_NODE:
				return (this as ProcessingInstruction).target;
			case Node.COMMENT_NODE:
				return '#comment';
			case Node.DOCUMENT_NODE:
				return '#document';
			case Node.DOCUMENT_FRAGMENT_NODE:
				return '#document-fragment';
			default:
				return nodeNameFallback?.get?.call(this) ?? '';
		}
	}
});

// happy-dom's node iterator stops at the first node its caller removes, and DOMPurify
// removes nodes while it walks. This iterator takes a snapshot in document order and
// skips the nodes that have left the root; DOMPurify captures it when it is created.
const isUnder = (root: Node, node: Node | null): boolean => {
	for (let current = node; current; current = current.parentNode) if (current === root) return true;
	return false;
};
document.createNodeIterator = ((root: Node, whatToShow: number) => {
	const nodes: Node[] = [];
	const collect = (node: Node) => {
		if (whatToShow & (1 << (node.nodeType - 1))) nodes.push(node);
		for (let child = node.firstChild; child; child = child.nextSibling) collect(child);
	};
	collect(root);
	let index = -1;
	return {
		root,
		nextNode() {
			while (++index < nodes.length) {
				if (isUnder(root, nodes[index])) return nodes[index];
			}
			return null;
		},
		detach() {}
	};
}) as unknown as Document['createNodeIterator'];

// Loaded after the shim: DOMPurify reads its accessors when its instances are created.
const { katexStyle, neutralizeCss, sanitizeKatex, sanitizeMarkdown, sanitizeSvg } = await import(
	'../../src/lib/features/conversation/markdown/sanitize'
);
const { MAX_TEX_LENGTH, loadKatex } = await import(
	'../../src/lib/features/conversation/markdown/math'
);
const {
	MAX_SOURCE_LENGTH,
	createMermaidCache,
	exportableSvg,
	mermaidSourceError,
	neutralizeMermaid,
	renderMermaid
} = await import('../../src/lib/features/conversation/markdown/mermaid');

function declarations(style: string): string[] {
	return style
		.split(';')
		.map((d) => d.replace(/\s+/g, ''))
		.filter((d) => d !== '')
		.sort();
}

function parse(html: string): HTMLElement {
	const root = document.createElement('div');
	root.innerHTML = html;
	return root;
}

describe('sanitizeMarkdown', () => {
	it('removes scripts, event handlers and images', () => {
		const out = sanitizeMarkdown(
			'<p onclick="x()">a</p><script>alert(1)</script><img src="x" onerror="alert(1)">'
		);
		expect(out).not.toMatch(/<script|onclick|onerror|<img/i);
		expect(out).toContain('<p>a</p>');
	});

	it('keeps http(s), mailto and in-page links only', () => {
		const out = parse(
			sanitizeMarkdown(
				[
					'<a href="javascript:alert(1)">j</a>',
					'<a href="data:text/html,x">d</a>',
					'<a href="vbscript:x">v</a>',
					'<a href="https://example.com/a">ok</a>',
					'<a href="mailto:a@b.c">m</a>',
					'<a href="#intro">frag</a>',
					'<a href="notes/a.md">rel</a>',
					'<a href="java&#x09;script:alert(1)">tab</a>'
				].join('')
			)
		);
		const hrefs = [...out.querySelectorAll('a')]
			.map((a) => a.getAttribute('href'))
			.filter((href) => href !== null);
		expect(hrefs).toEqual(['https://example.com/a', 'mailto:a@b.c', '#intro']);
	});

	it('drops style attributes and style elements', () => {
		const out = sanitizeMarkdown(
			'<p style="background:url(http://x)">s</p><style>p{background:url(x)}</style>'
		);
		expect(out).not.toMatch(/style|url\(/i);
	});

	it('keeps data attributes and button type=button', () => {
		const out = parse(sanitizeMarkdown('<button type="button" data-copy>Copier</button>'));
		const button = out.querySelector('button');
		expect(button?.getAttribute('type')).toBe('button');
		expect(button?.hasAttribute('data-copy')).toBe(true);
	});

	it('removes SVG foreign content, its links and its frames', () => {
		const out = sanitizeMarkdown(
			'<svg><foreignObject><iframe src="x"></iframe></foreignObject></svg>' +
				'<svg><a xlink:href="javascript:alert(1)"><text>x</text></a></svg>'
		);
		expect(out).not.toMatch(/<svg|foreignobject|iframe|javascript/i);
	});
});

describe('KaTeX output sanitizing', () => {
	it('keeps the declarations KaTeX writes and drops the others', () => {
		const out = katexStyle(
			'height:1em;background:url(http://x);position:fixed;color:red;transform:rotate(1deg);' +
				'vertical-align:-0.345em;margin-left:0em;width:expression(alert(1));@import url(x)'
		);
		expect(declarations(out)).toEqual([
			'color:red',
			'height:1em',
			'margin-left:0em',
			'vertical-align:-0.345em'
		]);
	});

	it('accepts position only as relative', () => {
		expect(katexStyle('position:relative')).toBe('position: relative');
		expect(katexStyle('position:fixed')).toBe('');
		expect(katexStyle('position:absolute')).toBe('');
	});

	it('refuses urls, expressions, escapes and unknown values', () => {
		for (const value of [
			'top:url(http://evil)',
			'top:expression(alert(1))',
			'top:\\75 rl(http://evil)',
			'color:red!important',
			'color:javascript:alert(1)'
		]) {
			expect(katexStyle(value), value).toBe('');
		}
	});

	it('removes handlers, links, external references and animations', () => {
		const out = sanitizeKatex(
			[
				'<span onclick="x()" style="width:expression(alert(1))">a</span>',
				'<a href="https://example.com">l</a>',
				'<svg><image href="http://evil/x.png"/><use href="http://evil#x"/></svg>',
				'<svg><path d="M0 0"/><animate attributeName="href" to="javascript:x"/></svg>',
				'<math><mi href="javascript:x">x</mi></math>',
				'<span style="top:url(http://evil)">b</span>'
			].join('')
		);
		expect(out).not.toMatch(/onclick|href|<image|<use|animate|javascript|evil|expression|url\(/i);
		expect(out).toContain('<path');
		expect(out).toContain('>a</span>');
	});

	it('keeps the real KaTeX output of common expressions', () => {
		const corpus = [
			'\\frac{a}{b}+x_1^2',
			'\\begin{pmatrix}a&b\\\\c&d\\end{pmatrix}',
			'\\sqrt{\\frac{a}{b}+\\frac{c}{d}}',
			'\\overbrace{x+y}^{n}',
			'\\overline{xy}',
			'\\rule{1em}{2em}',
			'\\textcolor{red}{x}',
			'\\colorbox{yellow}{y}',
			'\\fcolorbox{red}{blue}{z}',
			'\\boxed{x}',
			'\\begin{array}{|c|c|}\\hline a & b \\\\ \\hline\\end{array}',
			'\\left(\\frac12\\right)',
			'\\rlap{x}\\llap{y}'
		];
		for (const tex of corpus) {
			for (const displayMode of [false, true]) {
				const raw = katexLib.renderToString(tex, {
					output: 'htmlAndMathml',
					displayMode,
					trust: false,
					strict: 'ignore'
				});
				const clean = sanitizeKatex(raw);
				// The visible part (katex-html): every declaration KaTeX wrote survives,
				// except its own invalid `style="false"`. The MathML part is checked in a browser.
				const visible = (html: string) => html.slice(html.indexOf('class="katex-html"'));
				const rawStyles = [...visible(raw).matchAll(/style="([^"]*)"/g)].map((m) => m[1]);
				const cleanStyles = [...visible(clean).matchAll(/style="([^"]*)"/g)].map((m) => m[1]);
				const written = rawStyles.filter((s) => s !== 'false').flatMap(declarations).sort();
				expect(cleanStyles.flatMap(declarations).sort(), tex).toEqual(written);
				expect(clean, tex).not.toMatch(/<a\b|\shref=|<script|<foreignobject/i);
			}
		}
	});
});

describe('SVG output sanitizing (Mermaid)', () => {
	it('removes foreign content, scripts and handlers', () => {
		const foreign = sanitizeSvg('<svg><foreignObject><div>x</div></foreignObject></svg>');
		expect(foreign).not.toMatch(/foreignobject|<div/i);
		const script = sanitizeSvg('<svg><script>alert(1)</script></svg>');
		expect(script).not.toMatch(/script|alert/i);
		const handler = sanitizeSvg('<svg><g onload="x()"><rect width="1" height="1"/></g></svg>');
		expect(handler).not.toMatch(/onload/i);
		expect(handler).toContain('<rect');
	});

	it('keeps internal references, and web or mail links on anchors only', () => {
		const out = parse(
			sanitizeSvg(
				'<svg><a href="https://example.test/x"><text>a</text></a>' +
					'<a xlink:href="#node-1"><text>b</text></a>' +
					'<a href="mailto:me@example.test"><text>c</text></a>' +
					'<a href="javascript:alert(1)"><text>d</text></a>' +
					'<a href="data:text/html,x"><text>e</text></a>' +
					'<image href="http://evil/x.png"/></svg>'
			)
		);
		expect(out.innerHTML).not.toMatch(/evil|<image|javascript|data:/);
		expect(out.innerHTML).toContain('#node-1');
		expect(out.innerHTML).toContain('https://example.test/x');
		expect(out.innerHTML).toContain('mailto:me@example.test');
	});

	it('keeps internal url() references (markers, gradients) and drops external ones', () => {
		const out = sanitizeSvg(
			'<svg><path d="M0 0" marker-end="url(#arrowhead)" fill="url(http://evil/x)"/>' +
				'<rect fill="url(#grad)"/></svg>'
		);
		expect(out).toContain('marker-end="url(#arrowhead)"');
		expect(out).toContain('fill="url(#grad)"');
		expect(out).not.toContain('evil');
	});

	it('neutralizes the CSS of the diagram: url(), @import and expression() removed, rules kept', () => {
		const css = neutralizeCss(
			'.a{fill:url(http://evil)} @import url(x); #id .b{stroke:#f00} ' +
				'.c{width:expression(alert(1))} .d{fill:u\\72l(http://evil)} .e{fill:url(#grad)}'
		);
		expect(css).toContain('#id .b{stroke:#f00}');
		expect(css).toContain('url(#grad)');
		// No url() to another resource, no import, no expression, no escape left (`u\\72l(` is no function).
		expect(css).not.toMatch(/url\s*\(\s*['"]?(?!#)|@import|expression|\\/i);
		// The element itself is not parsed as SVG by happy-dom, so the markup check is weaker.
		expect(sanitizeSvg('<svg><style>.a{fill:url(http://evil)}</style></svg>')).not.toMatch(/evil/);
	});

	it('neutralizes the style attribute without losing its safe declarations', () => {
		const out = parse(
			sanitizeSvg('<svg><g style="fill:url(http://x);stroke:red"></g></svg>')
		);
		const style = out.querySelector('g')?.getAttribute('style') ?? '';
		expect(style).toContain('stroke:red');
		expect(style).not.toMatch(/url|http/);
	});

	it('cannot break out of a style element', () => {
		const out = sanitizeSvg('<svg><style>a{}</style><img src="x" onerror="alert(1)"></svg>');
		expect(out).not.toMatch(/<img|onerror/i);
	});
});

describe('KaTeX rendering (real library)', () => {
	it('renders the usual expressions, display and inline', async () => {
		const render = await loadKatex();
		const fraction = render('\\frac{a}{b}', true, {});
		expect('html' in fraction && fraction.html).toMatch(/frac-line/);
		const scripts = render('x_1^2', false, {});
		expect('html' in scripts && scripts.html).toMatch(/msupsub/);
		const matrix = render('\\begin{pmatrix}a&b\\\\c&d\\end{pmatrix}', true, {});
		expect('html' in matrix && (matrix.html.match(/col-align-c/g) ?? []).length).toBeGreaterThan(0);
	});

	it('returns the one-shared renderer', async () => {
		expect(await loadKatex()).toBe(await loadKatex());
	});

	it('refuses \\href and \\url (trust is off)', async () => {
		const render = await loadKatex();
		for (const tex of ['\\href{https://example.com}{x}', '\\url{https://example.com}']) {
			const result = render(tex, false, {});
			const html = 'html' in result ? result.html : '';
			expect(html, tex).not.toMatch(/<a\b|\shref=/);
		}
	});

	it('bounds macro expansion (maxExpand)', async () => {
		const render = await loadKatex();
		const started = Date.now();
		const result = render('\\def\\loop{\\loop\\loop}\\loop', false, {});
		expect('error' in result).toBe(true);
		expect(Date.now() - started).toBeLessThan(2000);
	});

	it('keeps a macro defined with \\gdef in its message table, and out of another table', async () => {
		const render = await loadKatex();
		const first: Record<string, string> = {};
		const second: Record<string, string> = {};
		expect('html' in render('\\gdef\\brand{Bricks}', false, first)).toBe(true);
		expect('html' in render('\\brand', false, first)).toBe(true);
		const other = render('\\brand', false, second);
		expect('error' in other).toBe(true);
	});

	it('keeps a \\def local to its expression', async () => {
		const render = await loadKatex();
		const table: Record<string, string> = {};
		expect('html' in render('\\def\\local{x}\\local', false, table)).toBe(true);
		expect('error' in render('\\local', false, table)).toBe(true);
	});

	it('returns a short error for a formula that does not parse, and refuses an overlong one', async () => {
		const render = await loadKatex();
		const broken = render('\\frac{', false, {});
		expect('error' in broken && broken.error.length).toBeGreaterThan(0);
		const long = render('x'.repeat(MAX_TEX_LENGTH + 1), false, {});
		expect('error' in long && long.error).toMatch(/trop longue/);
	});
});

describe('Mermaid bounds and neutralizing', () => {
	it('removes directives, closed or not', () => {
		const out = neutralizeMermaid(
			'%%{init: {"theme":\n "dark"}}%%\nflowchart TD\nA-->B\n%%{init: broken\nC-->D'
		);
		expect(out).not.toMatch(/%%\{|init|theme/);
		expect(out).toContain('flowchart TD');
		expect(out).toContain('C-->D');
	});

	it('removes the front matter at the top', () => {
		const out = neutralizeMermaid('---\nconfig:\n  securityLevel: loose\n---\nflowchart LR\nA-->B');
		expect(out.trimStart().startsWith('flowchart LR')).toBe(true);
		expect(out).not.toMatch(/securityLevel|config/);
	});

	it('removes callbacks, keeps web links and the edges', () => {
		const out = neutralizeMermaid(
			'flowchart TD\nA-->B\nclick A call alert\nclick B href "https://x.y"\nclick C "https://z"\nA --> C'
		);
		expect(out).not.toMatch(/call/);
		expect(out).toContain('click B href "https://x.y"');
		expect(out).toContain('click C "https://z"');
		expect(out).toContain('A-->B');
		expect(out).toContain('A --> C');
	});

	it('removes style statements that load or run something, keeps the safe ones', () => {
		const out = neutralizeMermaid(
			[
				'flowchart TD',
				'classDef ok fill:#f9f',
				'classDef bad fill:url(http://x)',
				'style A @import url(x)',
				'style B fill:#fff',
				'style C fill:expression(alert(1))',
				'linkStyle 0 stroke:url(http://evil)'
			].join('\n')
		);
		expect(out).toContain('classDef ok fill:#f9f');
		expect(out).toContain('style B fill:#fff');
		expect(out).not.toMatch(/bad|@import|expression|evil/);
	});

	it('refuses a source longer than the bound, before Mermaid sees it', async () => {
		expect(mermaidSourceError('x'.repeat(MAX_SOURCE_LENGTH))).toBeNull();
		expect(mermaidSourceError('x'.repeat(MAX_SOURCE_LENGTH + 1))).toMatch(/trop long/);
		const result = await renderMermaid('x'.repeat(MAX_SOURCE_LENGTH + 1), {
			id: 'mmd-big',
			theme: 'app', mode: 'light',
			fontFamily: 'sans-serif'
		});
		expect('error' in result && result.error).toMatch(/trop long/);
	});

	it('refuses an invalid identifier', async () => {
		const result = await renderMermaid('flowchart TD\nA-->B', {
			id: 'bad id!',
			theme: 'app', mode: 'light',
			fontFamily: 'sans-serif'
		});
		expect('error' in result && result.error).toMatch(/Identifiant/);
	});

	it('returns a clean, one-line error when the layout fails (happy-dom has no layout)', async () => {
		const result = await renderMermaid('flowchart TD\nA-->B', {
			id: 'mmd-test',
			theme: 'dark',
			fontFamily: 'sans-serif',
			colors: { background: '#000000', foreground: 'not a colour; } body{' }
		});
		if ('svg' in result) {
			expect(result.svg).toMatch(/^<svg/);
			return;
		}
		expect(result.error.length).toBeGreaterThan(0);
		expect(result.error.length).toBeLessThanOrEqual(200);
		expect(result.error).not.toContain('\n');
	});

	it('caches by key within its bound, least recently used out first', () => {
		const cache = createMermaidCache(2);
		cache.set('a', '<svg>a</svg>');
		cache.set('b', '<svg>b</svg>');
		expect(cache.get('a')).toBe('<svg>a</svg>');
		cache.set('c', '<svg>c</svg>');
		expect(cache.get('b')).toBeUndefined();
		expect(cache.get('a')).toBe('<svg>a</svg>');
		expect(cache.get('c')).toBe('<svg>c</svg>');
	});
});

describe('exportableSvg', () => {
	it('adds the prolog, the namespace and explicit dimensions from the viewBox', () => {
		const out = exportableSvg(
			'<svg id="mmd-1" width="100%" style="max-width: 300px;" viewBox="0 0 300 200" role="graphics-document"><g/></svg>'
		);
		expect(out.startsWith('<?xml version="1.0" encoding="UTF-8"?>\n<svg')).toBe(true);
		expect(out).toContain('xmlns="http://www.w3.org/2000/svg"');
		expect(out).toContain('width="300"');
		expect(out).toContain('height="200"');
		expect(out).toContain('style="max-width: 300px;"');
		expect(out.endsWith('</svg>')).toBe(true);
	});

	it('keeps an existing namespace and explicit dimensions', () => {
		const out = exportableSvg(
			'<?xml version="1.0"?>\n<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 400 200"></svg>'
		);
		expect(out.match(/xmlns=/g)?.length).toBe(1);
		expect(out).toContain('width="40"');
		expect(out).toContain('height="20"');
		expect(out.match(/<\?xml/g)?.length).toBe(1);
	});
});


// The same real DOMPurify instance as the SVG profile tests above.
const { svgFrame, mermaidSvg, SVG_CSP, MAX_SVG_LENGTH } = await import('../../src/lib/features/conversation/markdown/svg-viewer');
const { defaultDiagramSettings } = await import('../../src/lib/features/diagrams/settings');

describe('isolated SVG documents', () => {
  // Links and animations off: the strictest viewer.
  const options = { ...defaultDiagramSettings().diagramViewer, allowLinks: false, allowAnimations: false };
  const open = defaultDiagramSettings().diagramViewer;
  it('keeps web links when links are allowed: followed in the frame itself, without a target', () => {
    const frame = svgFrame('<svg><a href="https://example.test/doc" target="_top"><text>Doc</text></a><a href="#n1"><text>n</text></a></svg>', open, 'light');
    expect(open.allowLinks).toBe(true);
    expect(frame.svg).toContain('href="https://example.test/doc"');
    expect(frame.svg).toContain('rel="noopener noreferrer"');
    expect(frame.svg).not.toMatch(/target=|href="#n1"/);
  });
  it('lets the diagram move when animations are allowed, except for reduced motion', () => {
    const frame = svgFrame('<svg><text style="animation:dash 2s linear infinite">x</text></svg>', open, 'light');
    expect(frame.svg).toContain('animation:dash 2s linear infinite');
    expect(frame.document).toContain('@media (prefers-reduced-motion:reduce){*{animation:none!important');
    expect(frame.document).not.toMatch(/-webkit-user-select:none!important;animation:none/);
  });
  it('removes scripts, foreign HTML, event handlers, external resources and all links before display/export', () => {
    const hostile = `<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><foreignObject><iframe src="https://evil.test"></iframe></foreignObject><image href="https://evil.test/x"/><a href="https://evil.test"><text onclick="alert(1)">Label</text></a><animate attributeName="x" from="0" to="1"/></svg>`;
    const frame = svgFrame(hostile, options, 'light');
    expect(frame.svg).toMatch(/^<svg/);
    expect(frame.svg).not.toMatch(/script|foreignObject|iframe|https:|onclick|<a\b|<animate/i);
    expect(frame.document).toContain(SVG_CSP);
    expect(frame.document).toContain('user-select:none!important');
    expect(frame.document).toContain('animation:none!important');
  });
  it('prevents diagram CSS overriding disabled animations or selection, and toggles native tooltips', () => {
    const svg = '<svg><title>Safe tooltip</title><style>#x{user-select:text!important;animation:spin 1s infinite}</style><text id="x" style="user-select:text!important;animation:spin 1s infinite">label</text></svg>';
    const locked = svgFrame(svg, options, 'light');
    expect(locked.svg).toContain('<title>Safe tooltip</title>');
    expect(locked.svg).not.toMatch(/user-select:|animation:/);
    const changed = svgFrame(svg, { ...options, allowTooltips: false, allowSelection: true }, 'dark');
    expect(changed.svg).not.toContain('<title>');
    expect(changed.document).toContain('user-select:text!important');
    expect(changed.document).toContain('color-scheme:dark');
  });
  it('decodes Mermaid sandbox output as inert data then sanitizes it', () => {
    const payload = '<body><svg><text>Été</text><script>alert(1)</script></svg></body>';
    const encoded = btoa(String.fromCharCode(...new TextEncoder().encode(payload)));
    const result = mermaidSvg(`<iframe src="data:text/html;charset=UTF-8;base64,${encoded}" sandbox="allow-popups"></iframe>`);
    expect(result).toContain('Été');
    expect(result).not.toMatch(/iframe|script|sandbox/);
    expect(mermaidSvg('<iframe src="https://evil.test"></iframe>')).toBe('');
    expect(mermaidSvg('<iframe src="data:text/html;charset=UTF-8;base64,***"/>')).toBe('');
    expect(svgFrame('x'.repeat(MAX_SVG_LENGTH + 1), options, 'light').svg).toBe('');
  });
  it('reports the natural size of the drawing, so its height is a maximum and not a fixed box', () => {
    expect(svgFrame('<svg viewBox="0 0 240 120"><rect width="10" height="10"/></svg>', options, 'light').size).toEqual({ width: 240, height: 120 });
    expect(svgFrame('<svg width="80" height="40"><rect width="10" height="10"/></svg>', options, 'light').size).toEqual({ width: 80, height: 40 });
    expect(svgFrame('<svg width="100%"><rect width="10" height="10"/></svg>', options, 'light').size).toBeNull();
  });
  it('lets the app background show through for the app theme only', () => {
    const svg = '<svg viewBox="0 0 10 10"><rect width="10" height="10"/></svg>';
    expect(svgFrame(svg, options, 'dark', 'transparent').document).toContain('background:transparent');
    expect(svgFrame(svg, options, 'dark').document).toContain('background:#111827');
  });
});
