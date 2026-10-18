# Plan de migration (exemple fictif)

1. Préparation
   - Inventaire des services
     - Services HTTP internes
       - `billing-api` (v2, sans état)
         - Dépend de `ledger-core`
   - Sauvegarde des fichiers de configuration
2. Bascule
   - Passer le trafic en deux temps
   - Surveiller les erreurs 5xx

- Premier niveau, liste lâche :

  Paragraphe rattaché à l'élément, avec une seconde phrase.

- Deuxième élément, toujours lâche.

  - Sous-élément A
  - Sous-élément B

> Citation de niveau 1.
>
> > Citation de niveau 2 : une note interne.
> >
> > > Citation de niveau 3 : un avertissement.
> > >
> > > > Citation de niveau 4 : la plus profonde, avec une liste :
> > > > - point un
> > > > - point deux
> > > >   1. sous-point numéroté
> > > >   2. autre sous-point

- [ ] Tâche non cochée
- [x] Tâche cochée
  - [ ] Sous-tâche imbriquée
    - [x] Sous-sous-tâche terminée
