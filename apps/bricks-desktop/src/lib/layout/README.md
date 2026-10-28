# Layout and design system

Feature components plug into these components; keep the props stable.

## Tokens (`src/lib/theme/tokens.json`)

`src/app.css` is the stylesheet entry point. Edit theme defaults, fonts and shared text sizes in
`src/lib/theme/tokens.json`: `npm run tokens:generate` writes `src/styles/tokens.generated.css`
and `src/styles/fonts.css`. Vite generates them on development/build startup and watches the JSON
in development. `npm run tokens:check` verifies that checked-in outputs match their source.
The build intentionally updates these tracked generated files when the JSON changes; use
`tokens:check` for a read-only consistency check. On invalid JSON, development logs the error
and stays running so it can recover after an edit; production builds fail.

The interface uses locally bundled Google Sans Flex at weight 400 for body text; existing
medium/semibold headings keep their weights. Code and the terminal use Google Sans Code,
including a real italic face. Font sources and licences are in `src/assets/fonts/README.md`.

`src/styles/tailwind.css` exposes the tokens as utilities (`text-ui` = 13px,
`text-caption` = 11px, `text-code` = 13px, `text-compact` = 12.5px). These sizes set no line height;
existing `text-xs`, `text-sm` and explicit leading remain unchanged. The shared `cn` in
`src/lib/utils.ts` recognises these names as font sizes, so it preserves text colours when merging.

`src/styles/base.css` owns page defaults, keyboard focus, reduced motion and file icons.
`src/styles/syntax.css` shares highlighting between Markdown and the reader, retaining their
context-specific differences. Keep feature layout and feature-specific styles in their components.

`palette.ts` derives its default previews from the JSON. Runtime overrides still apply as CSS
variables; `resolved-theme.ts` reads the effective variables for the terminal and Mermaid.
CSS and runtime consumers read `--ui-font-family` / `--code-font-family`; the `--font-sans`
and `--font-mono` names are declared only in the Tailwind bridge.

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
  bind:leftWidth bind:rightWidth bind:terminalRatio
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
| `terminalRatio` | `number | null`, bindable, default `null` | Terminal share of the centre column (0.1–0.8); `null` until derived once from the legacy px height. |
| `onLayoutChange` | `(layout: ShellLayout) => void` | Debounced (300 ms). Fires after user resize or toggle, never after automatic changes. |

Limits: side panels 200–480 px; centre at least 480 px; terminal at least 120 px; conversation at least 240 px tall.

### Breakpoints (window width, `layout/constants.ts`)

| Width | Mode | Left | Right | Terminal |
| --- | --- | --- | --- | --- |
| 1200 px and up | `wide` | inline (if open) | inline (if open) | inline (if open) |
| 1000 to 1199 px | `medium` | inline (if open) | temporary Sheet (on toggle) | inline (if open) |
| under 1000 px | `narrow` | temporary Sheet (on toggle) | temporary Sheet (on toggle) | inline (if open) |

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
| `title` | `Snippet` | Centred slot (conversation title and project), stable whatever the number of right actions. |
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
