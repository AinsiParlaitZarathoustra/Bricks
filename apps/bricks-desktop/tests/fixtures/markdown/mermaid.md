# Diagrammes (exemple)

Flux de validation :

```mermaid
flowchart TD
    A[Message reçu] --> B{Texte complet ?}
    B -- Oui --> C[Rendu final]
    B -- Non --> D[Rendu partiel]
    D --> E((Attente du delta))
    E --> B
    C --> F[/Affichage/]
```

Échange entre services :

```mermaid
sequenceDiagram
    autonumber
    participant U as Utilisateur
    participant UI as Interface
    participant E as Moteur
    U->>UI: Envoie une question
    UI->>E: Requête streaming
    E-->>UI: Deltas successifs
    loop Chaque tick
        UI->>UI: renderMarkdown(préfixe)
    end
    E-->>UI: Fin du flux
    UI-->>U: Réponse affichée
```

Diagramme invalide (syntaxe cassée, doit échouer proprement) :

```mermaid
flowchart LR
    A --> --> B[[
    subgraph
```
