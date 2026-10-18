# Références de fichiers (exemple)

Le point d'entrée est [a.rs:3](src/a.rs#L3) et la fonction de parsing est dans [parser.rs:120-134](src/parser/parser.rs#L120-L134).

Référence nue : voir src/lib.rs:42-50 et aussi src/config/mod.rs:7.

Une URL qui ressemble à un chemin : https://x.y/src/a.rs:3 (ne doit pas devenir un fichier local).

Lien Markdown vers un fichier relatif : [README](README.md) et [le module](./src/lib.rs).

Chemin Windows indicatif : C:\Users\dev\projet\src\main.rs:10.

Correctif proposé :

```diff
diff --git a/src/a.rs b/src/a.rs
index 3f1c2aa..7b9d0e4 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,6 +1,8 @@
 pub fn answer() -> u32 {
-    41
+    let base = 40;
+    base + 2
 }
diff --git a/src/lib.rs b/src/lib.rs
index 0a9c1d2..c4e8f10 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -42,9 +42,9 @@ mod a;
-pub use a::answer;
+pub use a::answer as the_answer;
 pub fn version() -> &'static str {
-    "0.1.0"
+    "0.1.1"
 }
diff --git a/docs/notes.md b/docs/notes.md
--- a/docs/notes.md
+++ b/docs/notes.md
@@ -3 +3 @@
-Ancienne formulation.
+Nouvelle formulation, plus courte.
```
