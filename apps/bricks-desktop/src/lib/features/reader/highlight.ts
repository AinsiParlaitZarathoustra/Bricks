// Highlighted source split into lines: highlight.js works on the whole
// window (multi-line strings and comments keep their colour) and its HTML is
// cut at each newline, closing the spans open at the end of a line and
// opening them again on the next one. The result is escaped HTML from
// highlight.js only: the file's text never reaches the DOM unescaped.
import hljs from 'highlight.js/lib/common'

function escape(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

/** One HTML string per line of `text`. Unknown languages are escaped as plain text. */
export function highlightLines(text: string, language: string | null): string[] {
  const known = language !== null && hljs.getLanguage(language) !== undefined
  const html = known ? hljs.highlight(text, { language: language!, ignoreIllegals: true }).value : escape(text)
  const lines: string[] = []
  const open: string[] = []
  let current = ''
  const tag = /<span[^>]*>|<\/span>|\n/g
  let last = 0
  for (const m of html.matchAll(tag)) {
    current += html.slice(last, m.index)
    last = (m.index ?? 0) + m[0].length
    if (m[0] === '\n') {
      lines.push(current + '</span>'.repeat(open.length))
      current = open.join('')
    } else if (m[0] === '</span>') {
      open.pop()
      current += m[0]
    } else {
      open.push(m[0])
      current += m[0]
    }
  }
  current += html.slice(last)
  lines.push(current + '</span>'.repeat(open.length))
  return lines
}
