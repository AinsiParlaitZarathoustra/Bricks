<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Kbd } from '$lib/components/ui/kbd/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import { cn } from '$lib/utils.js';
	import type { IconComponent } from './types.js';

	interface Props {
		/** Stable accessible name, in French, e.g. "Panneau de gauche". Does not change with state. */
		ariaLabel: string;
		/** Tooltip shown while the panel is hidden (the action opens it), e.g. "Afficher le panneau de gauche". */
		tooltipShow: string;
		/** Tooltip shown while the panel is visible, e.g. "Masquer le panneau de gauche". */
		tooltipHide: string;
		/** Whether the panel is currently visible. Exposed as aria-pressed. */
		pressed: boolean;
		icon: IconComponent;
		onclick: () => void;
		/** Keyboard shortcut to show in the tooltip. Pass only when the shortcut really exists. */
		shortcut?: string;
		disabled?: boolean;
		class?: string;
	}

	let {
		ariaLabel,
		tooltipShow,
		tooltipHide,
		pressed,
		icon: Icon,
		onclick,
		shortcut,
		disabled = false,
		class: className
	}: Props = $props();
</script>

<Tooltip.Provider delayDuration={400}>
	<Tooltip.Root>
		<Tooltip.Trigger>
			{#snippet child({ props })}
				<Button
					{...props}
					variant="ghost"
					size="icon-sm"
					aria-label={ariaLabel}
					aria-pressed={pressed}
					{disabled}
					{onclick}
					class={cn(
						'text-muted-foreground hover:text-foreground aria-pressed:bg-accent aria-pressed:text-foreground',
						className
					)}
				>
					<Icon class="size-4" aria-hidden="true" />
				</Button>
			{/snippet}
		</Tooltip.Trigger>
		<Tooltip.Content side="bottom" sideOffset={6}>
			<span>{pressed ? tooltipHide : tooltipShow}</span>
			{#if shortcut}
				<Kbd>{shortcut}</Kbd>
			{/if}
		</Tooltip.Content>
	</Tooltip.Root>
</Tooltip.Provider>
