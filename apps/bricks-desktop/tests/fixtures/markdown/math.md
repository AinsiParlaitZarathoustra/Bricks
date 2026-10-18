# Notes mathématiques (exemple)

Dans une phrase, la variable $x$ vaut $x = 3$ et la somme $\sum_{i=1}^{n} i = \frac{n(n+1)}{2}$.

Le coût est de 5 $ et $10 au total, soit 15 $ pour deux ; le prix du café reste à 2 € (pas de maths ici).

Bloc multiligne :

$$
\int_0^1 x^2 \, dx
  = \left[ \frac{x^3}{3} \right]_0^1
  = \frac{1}{3}
$$

Forme LaTeX parenthésée : \(a^2 + b^2 = c^2\), puis un bloc \[ e^{i\pi} + 1 = 0 \].

Variante dans une liste :

- Aire : $\pi r^2$
- Périmètre : \(2\pi r\)
- Bloc :
  $$
  A = \frac{1}{2} b h
  $$

Code inline qui ressemble à des maths : `$x$`, `\(y\)` et `$$z$$` doivent rester du code.

```latex
\begin{align}
  f(x) &= x^2 + 2x + 1 \\
       &= (x + 1)^2
\end{align}
```

```math
\nabla \cdot \mathbf{E} = \frac{\rho}{\varepsilon_0}
```

Un texte avec un dollar isolé $ à la fin d'une phrase, puis du texte normal.
