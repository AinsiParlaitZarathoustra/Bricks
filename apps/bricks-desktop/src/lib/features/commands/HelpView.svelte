<script lang="ts">
  // `/help`: the commands come from the engine registry (the same list as the
  // palette, nothing is written here); the keys are the desktop interactions.
  import { onMount } from 'svelte'
  import { Button } from '$lib/components/ui/button'
  import { Kbd } from '$lib/components/ui/kbd'
  import type { InspectorState } from '$lib/state/ui.svelte'
  import { slashRegistry } from './slash-registry.svelte'
  import { usageLine } from './palette'

  // The host passes `{ args }`; this view needs none of them.
  let _props: { args?: InspectorState['args'] } = $props()

  onMount(() => {
    void slashRegistry.load()
  })

  const INTERACTIONS: { keys: string[]; text: string }[] = [
    { keys: ['Entrée'], text: 'Envoie le message. Un texte qui commence par / est une commande, jamais un prompt.' },
    { keys: ['Maj', 'Entrée'], text: 'Nouvelle ligne dans le message.' },
    { keys: ['Cmd', 'Entrée'], text: 'Envoie aussi le message (Ctrl+Entrée sous Windows et Linux).' },
    { keys: ['/'], text: 'Au début du message, ouvre la liste des commandes.' },
    { keys: ['↑', '↓'], text: 'Dans la liste des commandes, choisit une commande.' },
    { keys: ['Tab'], text: 'Dans la liste, complète le nom de la commande.' },
    { keys: ['Entrée'], text: 'Dans la liste, complète un nom incomplet ; sur un nom complet, exécute la commande.' },
    { keys: ['Échap'], text: 'Ferme la liste des commandes sans effacer le message.' },
    { keys: ['↑'], text: 'Dans un message vide (ou curseur au début), rappelle les demandes précédentes.' },
    { keys: ['↓'], text: 'Revient vers les demandes plus récentes, puis vers le brouillon en cours, qui n’est jamais effacé.' },
  ]
</script>

<div class="flex flex-col gap-5 text-sm">
  <section class="flex flex-col gap-2" aria-labelledby="help-commands">
    <h3 id="help-commands" class="text-sm font-semibold">Commandes</h3>

    {#if slashRegistry.state.status === 'loading'}
      <p role="status" class="text-muted-foreground">Chargement des commandes…</p>
    {:else if slashRegistry.state.status === 'error'}
      <div role="alert" class="flex flex-col items-start gap-2">
        <p class="text-destructive">Commandes indisponibles : {slashRegistry.state.message}</p>
        <Button variant="outline" size="sm" onclick={() => slashRegistry.retry()}>Réessayer</Button>
      </div>
    {:else}
      <ul class="flex flex-col divide-y">
        {#each slashRegistry.state.commands as cmd (cmd.name)}
          <li class="flex flex-col gap-0.5 py-2">
            <span class="font-mono font-medium">{usageLine(cmd)}</span>
            {#if cmd.aliases.length > 0}
              <span class="text-xs text-muted-foreground">
                alias : {cmd.aliases.map((a) => `/${a}`).join(', ')}
              </span>
            {/if}
            <span class="text-muted-foreground">{cmd.description}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="flex flex-col gap-2" aria-labelledby="help-keys">
    <h3 id="help-keys" class="text-sm font-semibold">Clavier et interactions</h3>
    <ul class="flex flex-col gap-2">
      {#each INTERACTIONS as item, i (i)}
        <li class="flex items-start gap-3">
          <span class="flex shrink-0 flex-wrap gap-1 pt-0.5">
            {#each item.keys as key, k (k)}
              <Kbd>{key}</Kbd>
            {/each}
          </span>
          <span class="text-muted-foreground">{item.text}</span>
        </li>
      {/each}
    </ul>
    <p class="text-muted-foreground">
      Une génération se termine avec le bouton Arrêter. Les demandes d’approbation s’affichent dans la
      conversation, avec leurs boutons de décision. Les boutons de la barre supérieure ouvrent et ferment les
      panneaux.
    </p>
  </section>
</div>
