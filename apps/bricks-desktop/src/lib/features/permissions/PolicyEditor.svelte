<script lang="ts">
	// Edits a permission policy: the profile, and for CUSTOM an effect per class of
	// action and per tool. Every change is handed to `onchange` as a whole policy;
	// the caller saves it and passes the policy in force back.
	import X from '@lucide/svelte/icons/x';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import * as Select from '$lib/components/ui/select';
	import type { PermissionEffect, SessionPolicy } from '$lib/ipc/engine';
	import {
		CLASS_ROWS,
		EFFECT_OPTIONS,
		PROFILE_OPTIONS,
		effectLabel,
		isPermissionEffect,
		isPermissionProfile,
		toolRules,
		withClass,
		withProfile,
		withTool
	} from './policy';

	interface Props {
		policy: SessionPolicy;
		/** Tool names offered for a rule of their own; empty: the name is typed. */
		tools?: readonly string[];
		disabled?: boolean;
		onchange: (policy: SessionPolicy) => void;
	}

	let { policy, tools = [], disabled = false, onchange }: Props = $props();

	const uid = $props.id();

	const profile = $derived(policy.chosen ? policy.profile : undefined);
	const selectedProfile = $derived(PROFILE_OPTIONS.find((o) => o.value === profile));
	const rules = $derived(toolRules(policy));
	const free = $derived(tools.filter((name) => !(name in policy.custom.tools)));

	let newTool = $state('');

	function chooseProfile(next: string | undefined) {
		if (!isPermissionProfile(next) || (policy.chosen && next === policy.profile)) return;
		onchange(withProfile(policy, next));
	}

	function chooseClass(key: (typeof CLASS_ROWS)[number]['key'], next: string | undefined) {
		if (!isPermissionEffect(next) || next === policy.custom[key]) return;
		onchange(withClass(policy, key, next));
	}

	function chooseTool(name: string, next: string | undefined) {
		if (!isPermissionEffect(next) || next === policy.custom.tools[name]) return;
		onchange(withTool(policy, name, next));
	}

	function addTool(name: string) {
		const trimmed = name.trim();
		if (!trimmed || trimmed in policy.custom.tools) return;
		onchange(withTool(policy, trimmed, 'ask'));
		newTool = '';
	}
</script>

{#snippet effectSelect(id: string, value: PermissionEffect, label: string, pick: (next: string | undefined) => void)}
	<Select.Root type="single" {value} onValueChange={pick} {disabled}>
		<Select.Trigger {id} class="w-36" aria-label={label}>
			<span class="truncate">{effectLabel(value)}</span>
		</Select.Trigger>
		<Select.Content>
			{#each EFFECT_OPTIONS as option (option.value)}
				<Select.Item value={option.value} label={option.label}>{option.label}</Select.Item>
			{/each}
		</Select.Content>
	</Select.Root>
{/snippet}

<div class="flex flex-col gap-4">
	<div class="flex flex-col gap-1.5">
		<Label for={`${uid}-profile`} class="text-xs text-muted-foreground">Profil</Label>
		<Select.Root type="single" value={profile} onValueChange={chooseProfile} {disabled}>
			<Select.Trigger id={`${uid}-profile`} class="w-full max-w-72">
				<span class="truncate">
					{selectedProfile ? `${selectedProfile.label} · ${selectedProfile.detail}` : 'À choisir'}
				</span>
			</Select.Trigger>
			<Select.Content>
				{#each PROFILE_OPTIONS as option (option.value)}
					<Select.Item value={option.value} label={option.label}>
						<span class="font-medium">{option.label}</span>
						<span class="text-muted-foreground">{option.detail}</span>
					</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
	</div>

	{#if profile === 'custom'}
		<div class="flex flex-col gap-2">
			<h4 class="text-xs font-medium text-muted-foreground">Par action</h4>
			<ul class="flex flex-col gap-2">
				{#each CLASS_ROWS as row (row.key)}
					<li class="flex items-center justify-between gap-3">
						<Label for={`${uid}-${row.key}`} class="text-sm font-normal">{row.label}</Label>
						{@render effectSelect(`${uid}-${row.key}`, policy.custom[row.key], row.label, (next) =>
							chooseClass(row.key, next)
						)}
					</li>
				{/each}
			</ul>
		</div>

		<div class="flex flex-col gap-2">
			<h4 class="text-xs font-medium text-muted-foreground">Par outil</h4>
			{#if rules.length > 0}
				<ul class="flex flex-col gap-2">
					{#each rules as rule (rule.name)}
						<li class="flex items-center justify-between gap-3">
							<span class="min-w-0 font-mono text-xs break-all">{rule.name}</span>
							<div class="flex items-center gap-1">
								{@render effectSelect(`${uid}-tool-${rule.name}`, rule.effect, rule.name, (next) =>
									chooseTool(rule.name, next)
								)}
								<Button
									size="icon"
									variant="ghost"
									class="size-8"
									aria-label={`Retirer la règle de ${rule.name}`}
									{disabled}
									onclick={() => onchange(withTool(policy, rule.name, null))}
								>
									<X class="size-4" aria-hidden="true" />
								</Button>
							</div>
						</li>
					{/each}
				</ul>
			{/if}
			{#if tools.length > 0}
				{#if free.length > 0}
					<Select.Root type="single" value="" onValueChange={(name) => name && addTool(name)} {disabled}>
						<Select.Trigger class="w-full max-w-72" aria-label="Ajouter une règle pour un outil">
							<span class="truncate text-muted-foreground">Ajouter un outil</span>
						</Select.Trigger>
						<Select.Content>
							{#each free as name (name)}
								<Select.Item value={name} label={name}>
									<span class="font-mono text-xs">{name}</span>
								</Select.Item>
							{/each}
						</Select.Content>
					</Select.Root>
				{/if}
			{:else}
				<form
					class="flex max-w-72 items-center gap-2"
					onsubmit={(e) => {
						e.preventDefault();
						addTool(newTool);
					}}
				>
					<Input
						bind:value={newTool}
						placeholder="Nom de l’outil"
						aria-label="Nom de l’outil"
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-xs"
						{disabled}
					/>
					<Button type="submit" size="sm" variant="outline" disabled={disabled || !newTool.trim()}>
						Ajouter
					</Button>
				</form>
			{/if}
		</div>
	{/if}
</div>
