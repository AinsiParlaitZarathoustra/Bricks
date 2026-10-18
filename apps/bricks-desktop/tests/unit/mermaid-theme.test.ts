// Theme variables of Mermaid diagrams: the colours Bricks accepts, the chart colours
// derived from them, and the text colour read on those charts. Pure functions only.
import { describe, expect, it } from 'vitest';
import {
	readableText,
	safeColors,
	themeVariables
} from '../../src/lib/features/conversation/markdown/mermaid';

type Vars = Record<string, unknown>;
const xy = (vars: Vars) => vars.xyChart as Record<string, string>;

describe('safeColors', () => {
	it('accepts the chart keys and the surface keys, as plain CSS colours', () => {
		expect(
			safeColors({ chart1: '#123456', chart5: 'rebeccapurple', background: 'hsl(0 0% 100%)' })
		).toEqual({ chart1: '#123456', chart5: 'rebeccapurple', background: 'hsl(0 0% 100%)' });
	});

	it('drops unknown keys and any value that is not a plain colour', () => {
		const safe = safeColors({
			chart2: '#abcdef',
			chart3: 'red; } body { background: url(http://x)',
			chart4: '#12',
			unknown: '#000000',
			background: 'javascript:alert(1)'
		});
		expect(safe).toEqual({ chart2: '#abcdef' });
	});

	it('is empty without colours', () => {
		expect(safeColors(undefined)).toEqual({});
	});
});

describe('themeVariables: charts', () => {
	it('uses the light chart palette for the light theme and the dark one for the dark theme', () => {
		expect(themeVariables('light', {}, 'sans-serif').pie1).toBe('#a65a35');
		expect(themeVariables('dark', {}, 'sans-serif').pie1).toBe('#e0936b');
	});

	it('cycles the five chart colours over the pies, the timelines and the branches', () => {
		const vars = themeVariables('light', {}, 'sans-serif');
		const charts = ['#a65a35', '#3b6db0', '#2f7a4d', '#9a6a1f', '#6d6a64'];
		for (let i = 0; i < 12; i++) {
			expect(vars[`pie${i + 1}`], `pie${i + 1}`).toBe(charts[i % 5]);
			expect(vars[`cScale${i}`], `cScale${i}`).toBe(charts[i % 5]);
		}
		for (let i = 0; i < 8; i++) expect(vars[`git${i}`], `git${i}`).toBe(charts[i % 5]);
		expect(vars.cScale11).toBe('#3b6db0');
		expect(vars.git7).toBe('#2f7a4d');
	});

	it('takes the chart colours given by the application', () => {
		const vars = themeVariables('light', { chart1: '#123456' }, 'sans-serif');
		expect(vars.pie1).toBe('#123456');
		expect(vars.pie6).toBe('#123456');
		expect(xy(vars).plotColorPalette.split(',')[0]).toBe('#123456');
	});

	it('gives the XY chart its palette, comma separated, and its axes', () => {
		const vars = themeVariables('dark', {}, 'sans-serif');
		expect(xy(vars).plotColorPalette).toBe('#e0936b,#8fb4ea,#79c995,#e3b562,#aaa7a1');
		expect(xy(vars).backgroundColor).toBe('#0d1117');
		expect(xy(vars).titleColor).toBe('#e6edf3');
		expect(xy(vars).xAxisLabelColor).toBe('#e6edf3');
		expect(xy(vars).yAxisLineColor).toBe('#8b949e');
		expect(xy(vars).xAxisTickColor).toBe('#8b949e');
	});
});

describe('themeVariables: pie texts and strokes', () => {
	it('strokes pies with the border colour and titles and legends with the foreground', () => {
		const vars = themeVariables('light', {}, 'sans-serif');
		expect(vars.pieStrokeColor).toBe('#d0d7de');
		expect(vars.pieOuterStrokeColor).toBe('#d0d7de');
		expect(vars.pieTitleTextColor).toBe('#1f2328');
		expect(vars.pieLegendTextColor).toBe('#1f2328');
	});

	it('reads the section text on the chart colours: white on the light charts, dark on the dark ones', () => {
		expect(themeVariables('light', {}, 'sans-serif').pieSectionTextColor).toBe('#ffffff');
		expect(themeVariables('dark', {}, 'sans-serif').pieSectionTextColor).toBe('#0d1117');
	});

	it('keeps the theme variables of the surfaces', () => {
		const vars = themeVariables('dark', {}, 'Inter');
		expect(vars.fontFamily).toBe('Inter');
		expect(vars.background).toBe('#0d1117');
		expect(vars.textColor).toBe('#e6edf3');
		expect(vars.lineColor).toBe('#8b949e');
	});
});

describe('readableText', () => {
	it('picks the candidate with the largest smallest contrast over the fills', () => {
		expect(readableText(['#a65a35', '#3b6db0'], ['#1f2328', '#ffffff'])).toBe('#ffffff');
		expect(readableText(['#e0936b', '#8fb4ea'], ['#e6edf3', '#0d1117'])).toBe('#0d1117');
	});

	it('keeps the first candidate on a tie and when a colour is not a hex value', () => {
		expect(readableText(['#777777'], ['#000000', '#000000'])).toBe('#000000');
		expect(readableText(['rebeccapurple'], ['#000000', '#ffffff'])).toBe('#000000');
		expect(readableText([], ['#ffffff', '#000000'])).toBe('#ffffff');
	});
});
