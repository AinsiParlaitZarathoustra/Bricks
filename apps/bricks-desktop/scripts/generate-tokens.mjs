import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const appRoot = fileURLToPath(new URL('../', import.meta.url));
const sourcePath = path.join(appRoot, 'src/lib/theme/tokens.json');
const outputs = {
  'src/styles/tokens.generated.css': generateTokensCss,
  'src/styles/fonts.css': generateFontsCss,
};

function cssValue(value) {
  if (Array.isArray(value) && value.length === 3 && value.every((part) => typeof part === 'number')) {
    return `oklch(${value.map(String).join(' ')})`;
  }
  if (typeof value === 'string') return value;
  throw new Error(`Unsupported token value: ${JSON.stringify(value)}`);
}

function declarations(values) {
  return Object.entries(values).map(([name, value]) => `\t--${name}: ${cssValue(value)};`).join('\n');
}

function generateTokensCss(tokens) {
  const shared = {
    'ui-font-family': fontFamily(tokens.fonts.ui),
    'code-font-family': fontFamily(tokens.fonts.code),
    'ui-font-weight': String(tokens.fonts.ui.defaultWeight),
    'ui-font-size': tokens.fontSizes.ui,
    'caption-font-size': tokens.fontSizes.caption,
    'code-font-size': tokens.fontSizes.code,
    'compact-font-size': tokens.fontSizes.compact,
  };
  const lightValues = {};
  for (const [name, value] of Object.entries(tokens.colors.light)) {
    lightValues[name] = value;
    if (name === 'ring') lightValues.radius = tokens.values.radius;
  }
  const values = { ...lightValues, ...shared };
  const darkValues = { ...tokens.colors.dark };
  return `/* Generated from src/lib/theme/tokens.json. Do not edit by hand. */\n:root {\n\tcolor-scheme: light;\n${declarations(values)}\n}\n\n.dark {\n\tcolor-scheme: dark;\n${declarations(darkValues)}\n}\n`;
}

function fontFamily(font) {
  return [`'${font.family}'`, ...font.fallbacks.map((family) => /[\s,]/.test(family) ? `'${family}'` : family)].join(', ');
}

function generateFontsCss(tokens) {
  const faces = [tokens.fonts.ui, tokens.fonts.code].flatMap((font) =>
    font.faces.map(({ file, style }) => `@font-face {\n\tfont-family: '${font.family}';\n\tsrc: url('../assets/fonts/${font.directory}/${file}') format('woff2');\n\tfont-weight: ${font.weight};\n\tfont-style: ${style};\n\tfont-display: ${tokens.fonts.display};\n}`),
  );
  return `/* Generated from src/lib/theme/tokens.json. Font files are bundled locally. */\n${faces.join('\n\n')}\n`;
}

export function generateTokens({ check = false } = {}) {
  const tokens = JSON.parse(readFileSync(sourcePath, 'utf8'));
  const stale = [];
  for (const [relativePath, render] of Object.entries(outputs)) {
    const destination = path.join(appRoot, relativePath);
    const expected = render(tokens);
    let current = '';
    try {
      current = readFileSync(destination, 'utf8');
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
    if (current !== expected) {
      stale.push(relativePath);
      if (!check) {
        mkdirSync(path.dirname(destination), { recursive: true });
        writeFileSync(destination, expected);
      }
    }
  }
  return stale;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const check = process.argv.includes('--check');
  const stale = generateTokens({ check });
  if (stale.length && check) {
    console.error(`Generated token files are stale: ${stale.join(', ')}`);
    process.exitCode = 1;
  } else if (stale.length) {
    console.log(`Generated token files: ${stale.join(', ')}`);
  }
}
