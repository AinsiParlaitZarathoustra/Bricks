# Layout and design system

Owner: Sprint 14 design agent. Other feature agents plug into these components; keep the props stable.

## Tokens (`src/app.css`)

Light values are on `:root`, dark values on `.dark` (set by `theme`). Use the Tailwind utilities.

| Role | Tokens (utilities) |
| --- | --- |
| Surfaces | `background`, `card`, `popover`, `muted`, `sidebar`, `secondary`, `accent` |
| Text | `foreground`, `muted-foreground`, `card-foreground`, `sidebar-foreground` |
| Brand (terracotta) | `primary`, `primary-foreground`, `ring` (focus), `sidebar-primary` |
| Lines | `border`, `input` (3:1 against background), `sidebar-border` |
| Errors and deletions | `destructive`, `destructive-foreground`, `diff-del`, `diff-del-bg` |
| Success and additions | `success`, `success-soft`, `diff-add`, `diff-add-bg` |
| Warning | `warning`, `warning-soft` |
| Info (running) | `info`, `info-soft` |
| Code | `code-bg`, `code-foreground`, font `font-mono` |
| Status dot idle | `status-idle` |

All text pairs meet WCAG AA (4.5:1). Focus uses a 2px `ring` outline on `:focus-visible`.
`prefers-reduced-motion` shortens animations and transitions to 0.01 ms globally.

## Theme (`src/lib/theme/theme.svelte.ts`)

- `initTheme(initial: ThemeMode = 'system'): () => void`: call once at startup with the saved preference. Follows OS changes. Returns a disposer.
- `setTheme(mode)`: applies the mode to `<html>` (`dark` class). It does not persist: the caller saves the preference.
- `theme.mode` (`'light' | 'dark' | 'system'`) and `theme.resolved` (`'light' | 'dark'`): reactive reads.
- `isThemeMode(value)`: validates a value read from prefs.

## App shell (`src/lib/layout/AppShell.svelte`)

Three zones: left navigation, centre conversation, right context, plus a bottom terminal inside the centre.

```svelte
<AppShell
  bind:leftOpen bind:rightOpen bind:terminalOpen
  bind:leftWidth bind:rightWidth bind:terminalHeight
  onLayoutChange={(layout) => savePrefs(layout)}
>
  {#snippet left()}…{/snippet}
  {#snippet center()}…{/snippet}
  {#snippet right()}…{/snippet}
  {#snippet terminal()}…{/snippet}
  {#snippet topbar(controls)}<TopBar {...controls} title={…} />{/snippet}   <!-- optional -->
</AppShell>
```

| Prop | Type | Notes |
| --- | --- | --- |
| `left`, `right`, `terminal` | `Snippet` | Optional. Missing pane: no column, and its toggle is hidden. |
| `center` | `Snippet` | Required. Always mounted, so composer state survives panel toggles. |
| `topbar` | `Snippet<[ShellControls]>` | Optional. Default: a plain `TopBar` with the toggles. |
| `leftOpen`, `rightOpen` | `boolean`, bindable, default `true` | User preference. Automatic collapses never change it. |
| `terminalOpen` | `boolean`, bindable, default `false` | User preference. |
| `leftWidth`, `rightWidth` | `number` px, bindable, defaults 260 and 280 | Preferred inline widths. |
| `terminalHeight` | `number` px, bindable, default 240 | Preferred terminal height. |
| `onLayoutChange` | `(layout: ShellLayout) => void` | Debounced (300 ms). Fires after user resize or toggle, never after automatic changes. |

Limits: side panels 200–480 px; centre at least 480 px; terminal at least 120 px; conversation at least 240 px tall.

### Breakpoints (window width, `layout/constants.ts`)

| Width | Mode | Left | Right | Terminal |
| --- | --- | --- | --- | --- |
| 1200 px and up | `wide` | inline (if open) | inline (if open) | inline (if open) |
| 800 to 1199 px | `medium` | inline (if open) | temporary Sheet (on toggle) | inline (if open) |
| under 800 px | `narrow` | temporary Sheet (on toggle) | temporary Sheet (on toggle) | inline (if open) |

- Temporary Sheets are closed whenever the mode changes. Opening one does not change the preference.
- Toggles in a forced mode act on the Sheet. In `wide` they flip the preference.
- `aria-pressed` reflects what is visible.

### Behaviour notes for feature agents

- The terminal pane is mounted only while open. Keep PTY sessions in state outside the component and reattach on mount.
- A panel that moves between inline and Sheet remounts. Keep its state outside the snippet.
- Resizing never moves focus: when a handle is dragged, focus returns to the element that had it.

## Landmarks and headings

- AppShell renders one visually hidden `h1` («Bricks»). Do not add another `h1` in the top bar title or in panels. Use `h2` for panel titles and `h3` for sections (`SectionCard` default).
- Landmarks: `header` (top bar), `main` «Conversation», `section` «Terminal» (only while open), `aside` «Contexte» on the right. The left column is a plain `div`: `NavigationPanel` owns the only `aside` on the left. Do not add a second `aside` around a panel.

## Top bar (`TopBar.svelte`)

| Prop | Type | Notes |
| --- | --- | --- |
| `title` | `Snippet` | Left slot (session name, breadcrumb). |
| `actions` | `Snippet` | Right slot, before the toggles. |
| `leftPressed`, `rightPressed`, `terminalPressed`, `changesPressed` | `boolean` | Toggle state. |
| `onToggleLeft`, `onToggleRight`, `onToggleTerminal`, `onToggleChanges` | `() => void` | A toggle is shown only when its callback is set. |

The header is a drag region for Tauri (`data-tauri-drag-region`). Labels are French and stable (`Panneau de gauche`, `Panneau de droite`, `Terminal`, `Modifications`).

## Primitives

- `PanelToggle.svelte`: icon button with tooltip. Props: `ariaLabel`, `tooltipShow`, `tooltipHide`, `pressed`, `icon`, `onclick`, `shortcut?` (only when the shortcut really exists), `disabled?`.
- `StatusDot.svelte`: props `status: 'idle' | 'running' | 'waiting' | 'success' | 'error' | 'cancelled'`, `label?`, `showLabel?`. Default French labels are exported as `STATUS_LABELS`. Cancelled is a hollow ring. Running pulses.
- `DiffStat.svelte`: props `additions`, `deletions`, `label?` (what the numbers count). `+N` in `diff-add`, `−N` in `diff-del`, with a screen-reader summary.
- `SectionCard.svelte`: collapsible card (`title`, `description?`, `count?`, `open` bindable, `headingLevel?` default 3, `actions?`, `children`). The title is a heading that wraps the toggle button.
- `EmptyState.svelte`: `title` (rendered as a heading, `headingLevel?` default 2), `description?`, `icon?`, `action?`.

Shared types: `types.ts` (`ShellLayout`, `ShellControls`, `IconComponent`, `HeadingLevel`). Geometry helpers are in `geometry.ts`. Breakpoints and limits are in `constants.ts`.

## Temporary

`LayoutPreview.svelte` shows placeholder zones only. It is for visual checks and must be deleted before delivery.
