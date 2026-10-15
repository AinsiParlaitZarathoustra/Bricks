<script lang="ts">
	import type { ApprovalRequest } from '$lib/ipc/engine';

	interface Props {
		/** Approvals waiting for a decision in the conversation. */
		approvals: ApprovalRequest[];
	}

	let { approvals }: Props = $props();
</script>

{#if approvals.length > 0}
	<section
		aria-labelledby="pending-approvals-title"
		class="rounded-lg border border-warning bg-warning-soft p-3 text-sm text-foreground"
	>
		<h2 id="pending-approvals-title" class="flex items-center gap-2 text-sm font-medium">
			Approbations en attente
			<span class="font-mono text-xs tabular-nums">{approvals.length}</span>
		</h2>
		<ul class="mt-2 flex flex-col gap-2">
			{#each approvals as approval (approval.approval_id)}
				<li class="flex min-w-0 flex-col gap-0.5 text-xs">
					<span class="font-mono font-medium break-all">{approval.tool}</span>
					<span class="text-pretty text-muted-foreground">{approval.description}</span>
					{#if approval.agent_id}
						<span class="text-muted-foreground">Demandée par un sous-agent.</span>
					{/if}
				</li>
			{/each}
		</ul>
		<p class="mt-2 text-xs text-pretty text-muted-foreground">
			Les décisions se prennent dans la conversation.
		</p>
	</section>
{/if}
