# Bricks 0.4.6 - Benchmarks du 8 octobre 2026

Mesures effectuees sur le checkout `dee981e`, avec corrections locales des
harnesses de benchmark. Le code du moteur n'a pas ete modifie pour ces mesures.

- Machine : Apple A18 Pro, 6 coeurs, 8 Gio de RAM, macOS 27.2, ARM64.
- Rust : 1.98.1 ; Python des frameworks : 3.12.13.
- Compilation workspace : release, `opt-level = "z"`, thin LTO, symboles retires.
- Standalone : release avec `CARGO_PROFILE_RELEASE_OPT_LEVEL=z` et
  `CARGO_PROFILE_RELEASE_LTO=thin`.
- Passages chronometres successifs, sans compilation concurrente. Le Mac reste
  en usage normal, avec des processus systeme, dont iCloud, en arriere-plan.
- LongMemEval, Terminal-Bench et recherche semantique exclus.
- Aucun appel a un modele reel. Les agents sont construits sans inference ;
  le rapport de tokens utilise un fournisseur simule.

## Demarrage CLI

50 executions de `--version`, apres 3 executions de chauffe. La taille porte
sur le fichier execute, et la memoire est le maximum resident d'une execution.
Les tailles et RSS sont exprimes en Mio (division par 1024 au carre).

| CLI | Version | Moyenne | Min - max | Binaire | RSS maximal |
|---|---|---:|---:|---:|---:|
| Bricks | 0.4.6 | 4.8 ms | 4.3 - 6.1 ms | 29.4 Mio | 7.5 Mio |
| Claude Code | 2.1.289 | 7.3 ms | 6.4 - 11.2 ms | 219.0 Mio | 24.6 Mio |
| Codex CLI | 0.150.1 | 8.6 ms | 7.5 - 11.9 ms | 218.4 Mio | 16.9 Mio |

Ces mesures ne portent pas sur le temps de reponse a un prompt.

## Outils Du Moteur

Suite standalone, 100 iterations par outil apres chauffe, fichiers temporaires.
Toutes les executions d'outils sont verifiees comme reussies.

| Outil | Moyenne | Min | Max |
|---|---:|---:|---:|
| Read | 0.118 ms | 0.111 ms | 0.378 ms |
| Write | 0.050 ms | 0.037 ms | 0.290 ms |
| Edit | 0.759 ms | 0.700 ms | 1.830 ms |
| Glob | 0.066 ms | 0.046 ms | 0.230 ms |
| Grep | 2.017 ms | 1.365 ms | 4.765 ms |
| Bash | 39.695 ms | 35.799 ms | 84.883 ms |
| std::fs::read | 0.010 ms | 0.010 ms | 0.012 ms |
| std::fs::write | 0.045 ms | 0.028 ms | 0.165 ms |

Le benchmark rapide termine egalement avec un code de sortie 0. Sur ses
50 iterations, les moyennes sont :

| Read | Write | Edit | Glob | Grep | Bash |
|---:|---:|---:|---:|---:|---:|
| 0.11 ms | 0.05 ms | 0.81 ms | 0.05 ms | 1.86 ms | 39.82 ms |

`Edit` remplace toutes les occurrences du texte repete dans le fichier.
Les anciens chiffres chronometraient un appel ambigu dont l'erreur etait
ignoree : ils ne sont pas directement comparables a ce scenario corrige.

Les ratios historiques entre une lecture interne et `claude --help` ne sont
pas repris : ces deux commandes n'effectuent pas la meme operation.

## Memoire Locale

Suite `memory_bench`, avec chauffe et fichiers temporaires. Il s'agit de
mesures locales de fichiers et de graphe, pas de LongMemEval.

| Operation | Moyenne | Min | Max |
|---|---:|---:|---:|
| Scan de 100 fichiers avec frontmatter | 1.84 ms | 1.46 ms | 3.69 ms |
| Chargement MEMORY.md | 15.4 us | 12.9 us | 118.9 us |
| Rappel texte, 100 fichiers | 2.03 ms | 1.58 ms | 4.20 ms |
| Ecriture graphe, 1 000 noeuds | 76.2 us/noeud | - | - |
| Rappel graphe, 1 000 noeuds | 1.36 ms | 1.18 ms | 4.99 ms |
| Requete par sujet | 114.5 us | 104.6 us | 145.3 us |
| Ecriture session | 35.9 us/entree | 25.5 us | 383.7 us |
| Chargement session, 100 entrees | 279.3 us | 237.7 us | 381.5 us |

## Frameworks Agents

Les crates `cersei-*` sont le moteur utilise par Bricks. Le harness construit
un agent minimal avec un outil, sans executer de tour de modele. Il mesure
1 000 instanciations, puis la memoire de 1 000 agents Rust ou 500 agents Python.
Les versions Python proviennent de `uv.lock`, installees dans le meme environnement.

| Framework | Version | Construction moyenne | Allocation mesuree par agent |
|---|---|---:|---:|
| Bricks / Cersei | 0.4.6 | 21.33 us | 71 703 octets |
| Agno | 2.5.17 | 6.02 us | 5 394 octets |
| LangGraph | 1.1.8 | 1 950.65 us | 30 344 octets |
| PydanticAI | 1.22.0 | 228.62 us | 8 196 octets |
| CrewAI | 1.14.2 | 7 476.15 us | 17 900 octets |

La memoire Rust est mesuree avec jemalloc ; Python utilise tracemalloc,
qui ne compte pas toutes les allocations natives. Les colonnes d'allocation
ne sont donc pas une comparaison equivalente de RAM totale.

Construction concurrente et maintien en vie de 1 000 agents :

| Framework | Duree totale | RSS maximal du processus |
|---|---:|---:|
| Bricks / Cersei | 20.43 ms | 98.5 Mio |
| Agno | 19.0 ms | 103.6 Mio |
| LangGraph | 2 101.1 ms | 168.7 Mio |
| PydanticAI | 262.1 ms | 107.2 Mio |
| CrewAI | 3 024.0 ms | 1 526.2 Mio |

Le RSS inclut le runtime, les imports et les pics des phases precedentes.
Les latences individuelles Rust excluent l'attente avant le debut de la tache ;
celles de Python incluent l'attente de `asyncio.to_thread`. Les ratios entre
leurs p50/p99 individuels ne constituent pas une comparaison equivalente.

Bricks atteint aussi le palier de **10 000 agents** : construction totale
197.37 ms, RSS maximal 647.3 Mio, construction individuelle p50 0.066 ms et
p99 1.048 ms. C'est le plus grand palier teste, pas une limite maximale trouvee.

Rappel sur graphe de 10 000 noeuds : 10 travailleurs x 100 requetes,
soit 1 000 echantillons. Moyenne 87.99 ms, p50 83.03 ms, p95 130.01 ms,
p99 226.49 ms. Le bandeau du binaire annonce encore 100 agents, mais le code
execute effectivement 10 travailleurs. Le palier 100 000 noeuds n'a pas ete
execute ; ses champs JSON a zero ont `samples = 0` et ne sont pas des mesures.
L'axe 5, recherche semantique, est desactive.

## Stress Et Suivi Des Tokens

| Suite | Controles reussis | Echecs |
|---|---:|---:|
| Infrastructure | 46 | 0 |
| Outils | 47 | 0 |
| Orchestration | 33 | 0 |
| Skills | 47 | 0 |
| Memoire | 83 | 0 |
| Total | 256 | 0 |

Rapport de tokens simule : 5 017 tokens d'entree, 714 tokens de sortie,
deux appels d'outils reussis (`Glob`, `Read`). Les trois verifications
de coherence passent : tokens, suivi du cout simule, nombre d'appels.
Ces tokens et tarifs sont definis par le simulateur ; ils ne mesurent ni
une consommation reelle ni la qualite d'un modele.

## Corrections Des Harnesses

- Le lanceur global part maintenant de la racine du depot et cible les packages
  Cargo explicitement. Les echecs de pipeline sont propages.
- Les benchmarks d'outils verifient les erreurs et utilisent `replace_all`
  dans le scenario Edit repete.
- Le benchmark rapide affiche une difference de durees signee, sans panic
  lorsque la mesure brute est plus lente.
- La suite outils attend les six outils shell actuels ; ses seuils de vitesse
  n'ont pas ete releves.
- Le fournisseur simule donne des noms et identifiants aux appels d'outils,
  utilise un petit projet temporaire et verifie leur succes.
- Les benchmarks Python acceptent `CERSEI_BENCH_OUT_DIR`, pour conserver
  les anciens resultats dans `bench/general-agents/results/`.

## Reproduction Et Sorties

Les sorties brutes de ce passage sont dans
`target/benchmarks/2026-10-08/`, avec les JSON comparatifs dans `general-agents/`.

```sh
cargo run --release -p cersei --example benchmark_io
cargo run --release -p cersei-memory --features graph --example memory_bench
CERSEI_BENCH_AXES=1,2,3,4 cargo run --release -p cersei-agent \
  --example general_agent_bench --features bench-full
python3 scripts/bench_cli.py --bricks target/release/bricks --iterations 50
CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_LTO=thin \
  cargo run --release --manifest-path examples/benchmark/Cargo.toml
```

Les cinq exemples `stress_*` et `usage_report` se lancent avec
`cargo run --release -p cersei --example NOM`.
Pour Python, depuis `bench/general-agents`, installer avec
`uv sync --locked --extra all --python 3.12`, puis executer successivement
`.venv/bin/python bench_agno.py`, `bench_langgraph.py`, `bench_pydantic_ai.py`
et `bench_crewai.py`. Les appels de modele ne font pas partie de ces suites.

Les anciens scripts `run_tool_bench_claude.sh` et `run_tool_bench_codex.sh`
ciblent encore `abstract`. Leurs mesures de demarrage, taille et RSS sont
couvertes ici par `bench_cli.py` avec Bricks. Leurs sous-commandes historiques
et scenarios agentiques avec modele reel n'ont pas ete portes ni executes.
