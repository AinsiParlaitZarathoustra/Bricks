<script lang="ts">
	// Settings of Bricks: a large dialog with a section list. Entry point mounted by App.svelte.
	import type { Component } from 'svelte';
	import { MediaQuery } from 'svelte/reactivity';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Tabs from '$lib/components/ui/tabs';
	import Palette from '@lucide/svelte/icons/palette';
	import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
	import Cpu from '@lucide/svelte/icons/cpu';
	import Sparkles from '@lucide/svelte/icons/sparkles';
	import AppWindow from '@lucide/svelte/icons/app-window';
	import AppearanceSettings from './AppearanceSettings.svelte';
	import GeneralSettings from './GeneralSettings.svelte';
	import ModelSettings from './ModelSettings.svelte';
	import SkillsSettings from './SkillsSettings.svelte';
	import ApplicationSettings from './ApplicationSettings.svelte';

	let { open = $bindable(false) }: { open?: boolean } = $props();

	type SectionId = 'appearance' | 'general' | 'models' | 'skills' | 'application';

	const SECTIONS: {
		id: SectionId;
		label: string;
		icon: Component<{ class?: string; 'aria-hidden'?: boolean | 'true' | 'false' }>;
	}[] = [
		{ id: 'appearance', label: 'Apparence', icon: Palette },
		{ id: 'general', label: 'Général', icon: SlidersHorizontal },
		{ id: 'models', label: 'Modèles', icon: Cpu },
		{ id: 'skills', label: 'Skills', icon: Sparkles },
		{ id: 'application', label: 'Application', icon: AppWindow }
	];

	let section = $state<SectionId>('appearance');
	/** Wide windows put the section list on the left; narrow ones put it on top. Matches Tailwind `sm`. */
	const wide = new MediaQuery('min-width: 40rem');
</script>

<Dialog.Root bind:open>
	<Dialog.Content
		class="flex h-[min(44rem,calc(100dvh-2rem))] w-[min(60rem,calc(100vw-2rem))] max-w-none flex-col gap-0 overflow-hidden p-0 sm:max-w-none"
	>
		<div class="flex flex-col gap-1 border-b border-border px-6 py-4 pr-14">
			<Dialog.Title class="text-base font-semibold text-foreground">Paramètres de Bricks</Dialog.Title>
			<Dialog.Description class="sr-only">
				Préférences de l’application. Les modifications sont enregistrées automatiquement.
			</Dialog.Description>
		</div>

		<Tabs.Root
			bind:value={section}
			orientation={wide.current ? 'vertical' : 'horizontal'}
			class="flex min-h-0 flex-1 flex-col sm:flex-row"
		>
			<Tabs.List
				variant="line"
				aria-label="Sections des paramètres"
				class="w-full shrink-0 justify-start gap-1 overflow-x-auto rounded-none border-b border-border bg-muted/40 p-2 sm:w-56 sm:gap-0.5 sm:border-r sm:border-b-0 sm:p-3"
			>
				{#each SECTIONS as item (item.id)}
					{@const Icon = item.icon}
					<Tabs.Trigger
						value={item.id}
						class="h-9 flex-none justify-start gap-2 rounded-md px-3 text-ui text-muted-foreground data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-xs"
					>
						<Icon class="size-4" aria-hidden="true" />
						{item.label}
					</Tabs.Trigger>
				{/each}
			</Tabs.List>

			<Tabs.Content value="appearance" class="min-h-0 flex-1 overflow-y-auto p-6">
				<AppearanceSettings />
			</Tabs.Content>
			<Tabs.Content value="general" class="min-h-0 flex-1 overflow-y-auto p-6">
				<GeneralSettings />
			</Tabs.Content>
			<Tabs.Content value="models" class="min-h-0 flex-1 overflow-y-auto p-6">
				<ModelSettings />
			</Tabs.Content>
			<Tabs.Content value="skills" class="min-h-0 flex-1 overflow-y-auto p-6">
				<SkillsSettings />
			</Tabs.Content>
			<Tabs.Content value="application" class="min-h-0 flex-1 overflow-y-auto p-6">
				<ApplicationSettings />
			</Tabs.Content>
		</Tabs.Root>
	</Dialog.Content>
</Dialog.Root>
