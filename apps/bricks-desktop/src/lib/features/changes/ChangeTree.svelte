<script lang="ts" generics="T">
	// The changed files as folders you can open, as shadcn's File Tree: a folder is a
	// Collapsible whose trigger is a ghost Button, a file is whatever `file` renders.
	// Folders start open. Folders use Lucide's outline icons; files keep Bricks's language icons.
	import type { Snippet } from 'svelte';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import * as Collapsible from '$lib/components/ui/collapsible';
	import { Button } from '$lib/components/ui/button';
	import FolderIcon from '@lucide/svelte/icons/folder';
	import FolderOpenIcon from '@lucide/svelte/icons/folder-open';
	import ChangeTree from './ChangeTree.svelte';
	import type { TreeNode } from './tree';

	interface Props {
		nodes: TreeNode<T>[];
		/** A file of the tree: the item and the name shown (its last path part). */
		file: Snippet<[T, string]>;
	}

	let { nodes, file }: Props = $props();

	let closed = $state<Record<string, boolean>>({});
</script>

<div class="flex min-w-0 flex-col">
	{#each nodes as node (node.path)}
		{#if node.type === 'folder'}
			<Collapsible.Root
				bind:open={() => !closed[node.path], (open) => (closed[node.path] = !open)}
				class="group/folder min-w-0"
			>
				<Collapsible.Trigger>
					{#snippet child({ props })}
						<Button {...props} variant="ghost" size="sm" class="w-full min-w-0 justify-start text-ui font-normal">
							<ChevronRightIcon
								data-icon="inline-start"
								class="transition-transform group-data-[state=open]/folder:rotate-90"
								aria-hidden="true"
							/>
							{#if closed[node.path]}
								<FolderIcon aria-hidden="true" />
							{:else}
								<FolderOpenIcon aria-hidden="true" />
							{/if}
							<span class="min-w-0 truncate" title={node.path}>{node.name}</span>
						</Button>
					{/snippet}
				</Collapsible.Trigger>
				<Collapsible.Content>
					<div class="ml-4 flex min-w-0 flex-col">
						<ChangeTree nodes={node.children} {file} />
					</div>
				</Collapsible.Content>
			</Collapsible.Root>
		{:else}
			{@render file(node.item, node.name)}
		{/if}
	{/each}
</div>
