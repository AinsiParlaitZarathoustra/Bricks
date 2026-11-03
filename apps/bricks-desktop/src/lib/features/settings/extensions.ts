// Labels of the Extensions section: the state of Computer Use and of its permissions.
import { t } from '$lib/i18n/i18n.svelte'
import type { ComputerUseStatus } from '$lib/ipc/bindings/ComputerUseStatus'
import type { PermissionState } from '$lib/ipc/bindings/PermissionState'

export function computerUseLabel(status: ComputerUseStatus): string {
  switch (status.state) {
    case 'unsupported':
      return t('settings.unavailableOnThisSystem')
    case 'missing':
      return t('settings.cuaDriverNotFound')
    case 'off':
      return t('settings.disabled')
    case 'starting':
      return t('settings.starting')
    case 'ready':
      return t('settings.activeTools', { count: status.tools })
    case 'error':
      return t('common.error')
  }
}

export function permissionLabel(state: PermissionState): string {
  switch (state) {
    case 'granted':
      return t('settings.allowed')
    case 'missing':
      return t('settings.notAllowed')
    case 'unknown':
      return t('settings.notChecked')
  }
}
