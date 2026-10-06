# cersei-web test corpus

All files here are **synthetic**: written for these tests, not captured from
live services or copied from third-party sites.

* `ddg/` — pages shaped like DuckDuckGo's HTML interface
  (`html.duckduckgo.com/html/`), using the class names that interface is
  known to use (`result`, `result__a`, `result__snippet`, `result--ad`,
  `no-results`, the anomaly/challenge form). They test the parser's four
  outcomes (results, no results, challenge with HTTP 200, unknown structure);
  they do not prove the live service still uses this markup. The `uddg`
  links reproduce DuckDuckGo's redirect format (`//duckduckgo.com/l/?uddg=…`).
* `pages/` — HTML/JSON pages covering an article, an API reference page,
  a page dominated by navigation, tables and code, relative links, Unicode
  text in several scripts, a JavaScript-only shell and a page without
  content. Their text was written for the tests (French and English); the
  facts they state (timeouts, versions) are fictional.

The `{{BASE}}` and `{{BASE_ENC}}` (percent-encoded) placeholders in
`ddg/results.html` are replaced at run time by the address of the local test
server.
