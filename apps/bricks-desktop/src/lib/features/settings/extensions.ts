// Labels of the Extensions section: the state of Computer Use and of its permissions.
import type { ComputerUseStatus } from '$lib/ipc/bindings/ComputerUseStatus'
import type { PermissionState } from '$lib/ipc/bindings/PermissionState'

export function computerUseLabel(status: ComputerUseStatus): string {
  switch (status.state) {
    case 'unsupported':
      return 'Indisponible sur ce système'
    case 'missing':
      return 'Cua Driver introuvable'
    case 'off':
      return 'Désactivé'
    case 'starting':
      return 'Démarrage…'
    case 'ready':
      return status.tools === 1 ? 'Actif · 1 outil' : `Actif · ${status.tools} outils`
    case 'error':
      return 'Erreur'
  }
}

export function permissionLabel(state: PermissionState): string {
  switch (state) {
    case 'granted':
      return 'Autorisée'
    case 'missing':
      return 'Non autorisée'
    case 'unknown':
      return 'Non vérifiée'
  }
}
