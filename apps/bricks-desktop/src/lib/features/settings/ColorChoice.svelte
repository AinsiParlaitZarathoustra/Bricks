<script lang="ts">
	// One colour choice of the appearance settings: its current value, the button that opens the
	// palette, and the reset to the theme token.
	import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
	import { Button } from '$lib/components/ui/button';
	import * as Popover from '$lib/components/ui/popover';
	import type { ColorCheck, Hex, Swatch } from '$lib/theme/palette';
	import ColorPicker from './ColorPicker.svelte';

	let {
		label,
		value,
		custom,
		foreground,
		sample,
		presets,
		checks,
		onChange,
		onReset
	}: {
		label: string;
		/** The colour in use for this mode: the custom value, or the theme's. */
		value: Hex;
		/** Whether the value is set by the user (else it is the theme's token). */
		custom: boolean;
		foreground: Hex;
		sample: string;
		presets: readonly Swatch[];
		checks: ColorCheck[];
		onChange: (hex: Hex) => void;
		onReset: () => void;
	} = $props();

	let open = $state(false);
</script>

<div class="flex items-center justify-between gap-3 p-3">
	<div class="flex min-w-0 flex-col gap-0.5">
		<span class="text-ui font-medium text-foreground">{label}</span>
		<span class="font-mono text-xs text-muted-foreground tabular-nums">
			{custom ? value : `${value}, couleur du thème`}
		</span>
	</div>

	<div class="flex shrink-0 items-center gap-1">
		<Button variant="ghost" size="sm" disabled={!custom} onclick={onReset} aria-label={`Réinitialiser : ${label}`}>
			<RotateCcw aria-hidden="true" />
			Réinitialiser
		</Button>

		<Popover.Root bind:open>
			<Popover.Trigger>
				{#snippet child({ props })}
					<button
						{...props}
						type="button"
						class="size-8 shrink-0 rounded-md border border-border shadow-xs focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
						style:background-color={value}
						aria-label={`Modifier la couleur : ${label}, ${value}`}
					></button>
				{/snippet}
			</Popover.Trigger>
			<Popover.Content align="end" class="w-72 gap-3 p-3">
				<ColorPicker {label} {value} {foreground} {sample} {presets} {checks} {onChange} />
			</Popover.Content>
		</Popover.Root>
	</div>
</div>
