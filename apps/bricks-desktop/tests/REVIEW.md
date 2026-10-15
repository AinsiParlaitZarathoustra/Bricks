# Accessibility and responsive review

Reviewer: Haiku 8 (Sprint 14). Method: code reading of the layout and feature components as they
stood when this review ran (the feature components were still being written by other agents). Not
done: a run of the Tauri app, a screen-reader pass, a keyboard-only pass, contrast measurement. Each
item below is a reading of the source and should be confirmed in the running app before the fix lands.

Files read: `layout/*` (AppShell, TopBar, PanelToggle, StatusDot, DiffStat, EmptyState, SectionCard,
geometry, constants), `navigation/*`, `conversation/*` (title, timeline, markdown, timeline cards),
`composer/*`, `context/*`, `agents/*`, `changes/*` (panel, diff, file row, git and history scan),
`terminal/*`, and `src/app.css`, `index.html`, `src-tauri/tauri.conf.json`.

Severity: **High** = blocks a keyboard or assistive-technology user from a task or breaks ARIA
structure. **Medium** = fails a WCAG 2.1 AA criterion or hurts a common flow. **Low** = polish.

## Findings

| ID | Sev. | Where | Problem | Concrete fix |
| --- | --- | --- | --- | --- |
| R1 | High | `features/terminal/TerminalPanel.svelte:68-97` | `role="tablist"` contains a `role="presentation"` wrapper around a `role="tab"` button and a separate close `<button>`. A tablist may only own tabs, so the close buttons are invalid there. There is no arrow-key handling and no `aria-controls`. | Drop the tab roles: wrap the list in `role="group" aria-label="Terminaux du projet"`, make each name button a plain button with `aria-pressed={isActive}`, keep the close button as a sibling. If the tabs pattern is wanted instead, use roving `tabindex`, Arrow/Home/End, and `role="tabpanel"` on the terminal container. |
| R2 | High | `features/terminal/terminals.svelte.ts:51-57` (xterm options) and `TerminalPanel.svelte:117` | xterm is created without `screenReaderMode`, so terminal output is not exposed to assistive technology. The container has no name. | Set `screenReaderMode: true` in the `new Terminal({...})` options (confirm the option still exists in `@xterm/xterm` 6 and read its performance note), and give the container `role="group"` with `aria-label="Terminal {n}"`. If not done this sprint, record it as a known limit in the release notes. |
| R3 | High | `layout/AppShell.svelte:272` and `:315` wrap `features/navigation/NavigationPanel.svelte:36` | Nested landmarks: AppShell's `<aside aria-label="Navigation">` (inline and Sheet) wraps NavigationPanel's own `<aside aria-label="Projets et conversations">`. Two complementary landmarks nest, and the outer one is labelled wrongly. | In AppShell, replace the two `<aside aria-label="Navigation">` wrappers with a plain `<div>` that keeps the same classes, and keep NavigationPanel's `aside` as the only landmark. The right side is fine: ContextPanel has no inner aside.
| R4 | Medium | `features/conversation/timeline/MonoBlock.svelte:30` | Scroll container (`max-h-80 overflow-auto`) has no focusable element and no name. Keyboard users cannot scroll long code or tool output. | Add `role="region" aria-label="Sortie"` (or the block's label) and `tabindex="0"` to that div, with the focus ring class. |
| R5 | Medium | `features/changes/DiffView.svelte:52` | `role="region" aria-label="Diff"` scroll area is not focusable, so the same keyboard gap as R4. | Add `tabindex="0"` to the region. |
| R6 | Medium | `features/agents/AgentInspector.svelte:91` | `max-h-[62vh] overflow-y-auto` body of the dialog. Its focusable controls are reachable, but the region itself is not, and it has no name. | Add `tabindex="0"` and `aria-label="Détails du sous-agent"`, or move the scroll to the Dialog content. |
| R7 | Medium | `layout/SectionCard.svelte:36-47` and `layout/EmptyState.svelte:26` | Section titles (« Contexte », « Environnement », « Sous-agents ») are `span`s inside the collapsible button; empty-state titles are `p`. Screen-reader users cannot jump by heading. The app has no `h1` at all (`ConversationTitle.svelte:71-83` renders the title as a plain button or span). | Wrap the `Collapsible.Trigger` in `<h3 class="contents">` (keeps the layout, valid because a button is phrasing content). Render the empty-state title as `<h2 class="…">` (or pass the level as a prop). Render the conversation title as `<h1>` (or add `<h1 class="sr-only">Bricks</h1>` in AppShell). Keep one level per region: panel titles h2, sections h3. |
| R8 | Medium | `features/composer/ModelSelect.svelte:52`, `features/composer/EffortSelect.svelte:54` | `aria-label="Modèle"` and `aria-label="Effort de raisonnement"` replace the visible value (`Claude X`, `Rapide`). The name does not contain the visible text (WCAG 2.5.3) and the current choice is not announced. | `aria-label={\`Modèle : ${label}\`}` and `aria-label={\`Effort de raisonnement : ${triggerLabel}\`}`, so the name starts with the visible label and includes the value. |
| R9 | Medium | `features/composer/ProjectStrip.svelte:84` | `Tooltip.Trigger` renders a focusable `button` with no action; it only shows the project path. A tab stop that does nothing, which the rules forbid. | Render the trigger as a non-focusable `span` with `title={project.path}` (and keep the visible name), or make it a real action (open the project folder) once that exists. |
| R10 | Medium | `features/changes/ChangesPanel.svelte:16-21` | Three tabs in `grid-cols-3` inside the right panel (200 to 480 px). At the default 280 px width each tab gets about 77 px, so « Historique du projet » is probably truncated (estimated from the layout, not measured). | Shorten the visible label to « Historique » and keep the full name as `aria-label` on the trigger, or let the label wrap (`h-auto py-1.5 whitespace-normal`). |
| R11 | Medium | `features/agents/AgentInspector.svelte:251-268` | « Appliquer » writes the sub-agent's changes into the workspace and « Abandonner » drops them, both at once, with no confirmation. Other destructive actions in the app use an AlertDialog. | Confirm « Abandonner » with an AlertDialog (name the files and the agent). Confirm « Appliquer » too, or show the same file list in the confirmation. |
| R12 | Low | `features/conversation/timeline/ApprovalCard.svelte:97` | Several « Voir le diff » triggers in one card have the same name. Ambiguous for screen-reader users. | `aria-label={\`Voir le diff de ${file.path}\`}` (the name starts with the visible text, WCAG 2.5.3). |
| R13 | Low | `features/conversation/ConversationTitle.svelte:74` and `:79` | Two controls with the same name « Renommer la conversation » (the title button and the pencil). | Remove the pencil button (the title button already renames), or give it a different name such as « Modifier le titre ». |
| R14 | Low | `features/agents/AgentInspector.svelte:104` | « Annuler » cancels a running sub-agent, while « Annuler » in dialogs means « close ». | Rename to « Arrêter l’agent », matching the composer's « Arrêter ». |
| R15 | Low | `features/agents/AgentInspector.svelte:348-349` | « 12 entrée(s) · 3 sortie(s) » reads badly in French. | « 12 jetons en entrée · 3 jetons en sortie », with the plural handled by `plural()`. |
| R16 | Low | `features/context/ContextCard.svelte:86`, `features/context/ProjectEnvironment.svelte:146`, `features/agents/AgentInspector.svelte:221` | `text-[11px]` for paths and method names. Below the 12 px the rest of the UI uses. | Use `text-xs` (12 px) and let the value truncate. |
| R17 | Low | `features/context/ContextPanel.svelte:28`, `features/navigation/NavigationPanel.svelte` (ScrollArea) | shadcn `ScrollArea` viewports are not keyboard scrollable when their content has no focusable element (for example a long context card with plain text). | Give the viewport `tabindex="0"` and a name (`aria-label="Contexte du projet"`), or keep focusable content at the top. Verify in WebKit before changing. |

## Checked and fine (no change proposed)

- **Markdown and untrusted content.** Assistant markdown goes through DOMPurify (`features/conversation/markdown.ts:84`); links are intercepted and only `http`, `https` and `mailto` open. User text, tool output and notices are rendered as text nodes. `{@html}` is used only in `AssistantMessage.svelte:48`, on the sanitized output.
- **Live updates.** The timeline uses `role="log"` with `aria-live="off"` on purpose and a separate polite region that announces only completed answers, approval requests and errors (`ConversationTimeline.svelte:173`, `:228`). Streaming does not flood the screen reader.
- **Focus.** `:focus-visible` outline is global (`src/app.css:216`). Dialogs, sheets and menus use bits-ui focus trapping. Icon-only buttons carry names (PanelToggle, attachment remove, copy, dropdown triggers).
- **Motion.** `prefers-reduced-motion` stops infinite animations after one iteration (`src/app.css:229`); the streaming caret is also disabled (`AssistantMessage.svelte:187`).
- **Status and colour.** Status dots pair a colour with text (`StatusDot`), cancelled uses a hollow ring, diff rows also show `+`/`−` signs.
- **Responsive layout.** Breakpoints follow `layout/constants.ts`: 1200 px and up all inline; 800 to 1199 px right panel becomes a Sheet; under 800 px both become Sheets. The minimum window (640 × 480, `tauri.conf.json`) keeps the composer and the send action visible. Tables and code scroll inside their own containers rather than the page.
- **Settings dialog (this sprint).** Checked in a temporary browser harness at 800 × 600 and at 400 px wide, in light and dark: section list stacks on narrow widths, theme switch updates `<html>` and the checked radio, tables have captions and row headers, config errors show in a `role="alert"`. Keyboard use of the tabs and the radios was not tested in the browser (they use the bits-ui tab pattern and native radios). The harness was removed afterwards.

## Notes for the orchestrator

- R1 and R2 are the only findings that change the terminal's ARIA structure or its speech output. Their owner is the terminal feature (Haiku 7's area per the file headers). R3 and R7 touch the shell and the shared layout primitives.
- The manual checklist in `tests/README.md` describes some controls more loosely than the code does. Those lines were corrected in the same change.
- Nothing here was verified with a screen reader or in WebKit. Re-run this review once the components settle.
