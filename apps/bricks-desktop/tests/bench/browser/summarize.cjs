// Prints the browser equivalence results of tests/bench/results/browser/dom.html (run.sh first).
const fs = require('node:fs')
const path = require('node:path')
const file = path.join(__dirname, '..', 'results', 'browser', 'dom.html')
const html = fs.readFileSync(file, 'utf8')
const match = /<script type="application\/json" id="result">([\s\S]*?)<\/script>/.exec(html)
if (!match) throw new Error('no result in dom.html')
const r = JSON.parse(match[1])
console.log('UA', r.userAgent)
console.log('DOMPurify sanity (<p>a</p>\\n<p>b</p>):', JSON.stringify(r.domPurifySanity))
for (const sec of ['ordinary', 'newSyntax']) {
  for (const [k, v] of Object.entries(r[sec])) {
    console.log(sec.padEnd(9), k.padEnd(30), `strict=${v.equal ? 'EQ' : 'DIFF'}`, `modCopyType=${v.equalModuloCopyButtonType ? 'EQ' : 'DIFF'}`, `nonHtml=${v.nonHtmlBlocks}`, `items=${v.baselineItems}/${v.referenceItems}`)
  }
}
console.log('\n-- ordinary, residual diffs modulo copy-button type:')
for (const [k, v] of Object.entries(r.ordinary)) if (!v.equalModuloCopyButtonType) console.log(k, JSON.stringify(v.diffModuloCopyButtonType).slice(0, 900))
console.log('\n-- multiset diffs (new syntax):')
for (const [k, v] of Object.entries(r.newSyntax)) console.log(k, JSON.stringify(v.multiset))
console.log('\n-- multiset diffs (ordinary, not equal modulo copy-button type):')
for (const [k, v] of Object.entries(r.ordinary)) if (!v.equalModuloCopyButtonType) console.log(k, JSON.stringify(v.multiset))
