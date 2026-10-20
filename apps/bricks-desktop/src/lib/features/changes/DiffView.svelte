<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { cn } from '$lib/utils.js';
	import { parsePatch, type PatchLineKind } from './patch';

	interface Props {
		/** Unified diff text, as produced by Rust. */
		patch: string;
		/** The patch was cut to the size limit. */
		truncated?: boolean;
		/** Binary file: there is no text to show. */
		binary?: boolean;
		/** Lines added per « Afficher plus ». */
		pageSize?: number;
		/** A patch written in a message: `+`/`-` lines count even without hunk headers. */
		loose?: boolean;
		class?: string;
	}

	let { patch, truncated = false, binary = false, pageSize = 400, loose = false, class: className }: Props = $props();

	const lines = $derived(parsePatch(patch, { loose }));
	let extra = $state(0);
	const limit = $derived(pageSize + extra);
	const shown = $derived(lines.slice(0, limit));
	const hidden = $derived(lines.length - shown.length);

	// Both markers are on the left: a coloured gutter edge, then the old and
	// new line numbers; the line keeps its own `+` / `-` (not colour only).
	const ROW: Record<PatchLineKind, string> = {
		add: 'bg-diff-add-bg border-l-diff-add',
		del: 'bg-diff-del-bg border-l-diff-del',
		context: 'border-l-transparent',
		hunk: 'bg-muted text-muted-foreground border-l-transparent',
		meta: 'text-muted-foreground border-l-transparent'
	};
</script>

<div
	class={cn(
		'overflow-hidden rounded-md border border-border bg-code-bg text-xs text-code-foreground',
		className
	)}
>
	{#if truncated}
		<p role="status" class="border-b border-border px-2 py-1 text-warning">
			Diff tronqué : seules les premières lignes sont affichées (512 Kio au maximum).
		</p>
	{/if}

	{#if binary}
		<p class="px-2 py-2 text-muted-foreground">Fichier binaire : aperçu indisponible.</p>
	{:else if lines.length === 0}
		<p class="px-2 py-2 text-muted-foreground">Aucune modification de texte.</p>
	{:else}
		<!-- A named, focusable scroll container: keyboard users must be able to scroll
			it (WCAG 2.1.1). The rule only checks the role, so it is suppressed here. -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div
			class="max-h-96 overflow-auto rounded-sm font-mono leading-5 outline-none focus-visible:ring-2 focus-visible:ring-ring"
			role="region"
			aria-label="Diff"
			tabindex="0"
		>
			<div class="min-w-max py-1">
				{#each shown as line, i (i)}
					<div class={cn('grid grid-cols-[3rem_3rem_minmax(0,1fr)] border-l-[3px]', ROW[line.kind])}>
						<span class="px-1 text-right text-muted-foreground tabular-nums select-none">{line.oldNo ?? ''}</span>
						<span class="px-1 text-right text-muted-foreground tabular-nums select-none">{line.newNo ?? ''}</span>
						<span class="px-2 whitespace-pre">{line.text}</span>
					</div>
				{/each}
			</div>
		</div>
		{#if hidden > 0}
			<div class="border-t border-border px-2 py-1">
				<Button variant="ghost" size="xs" onclick={() => (extra += pageSize)}>
					Afficher plus ({hidden} lignes restantes)
				</Button>
			</div>
		{/if}
	{/if}
</div>
