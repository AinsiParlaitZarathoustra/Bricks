// The icon of each tool, by its real name. A call's state is said in words only
// when it failed; the icon says what the call does. Pure: unit-tested
// (tests/unit/tool-icons.test.ts).
import BinocularsIcon from '@lucide/svelte/icons/binoculars'
import BookOpenCheckIcon from '@lucide/svelte/icons/book-open-check'
import BotIcon from '@lucide/svelte/icons/bot'
import BracesIcon from '@lucide/svelte/icons/braces'
import BrainIcon from '@lucide/svelte/icons/brain'
import ChartLineIcon from '@lucide/svelte/icons/chart-line'
import ChevronRightIcon from '@lucide/svelte/icons/chevron-right'
import CodeIcon from '@lucide/svelte/icons/code'
import CpuIcon from '@lucide/svelte/icons/cpu'
import DatabaseIcon from '@lucide/svelte/icons/database'
import FileDiffIcon from '@lucide/svelte/icons/file-diff'
import FolderSearchIcon from '@lucide/svelte/icons/folder-search'
import GlobeIcon from '@lucide/svelte/icons/globe'
import ImageIcon from '@lucide/svelte/icons/image'
import ListTodoIcon from '@lucide/svelte/icons/list-todo'
import MessageSquareIcon from '@lucide/svelte/icons/message-square'
import MessageCircleQuestionIcon from '@lucide/svelte/icons/message-circle-question-mark'
import PackageOpenIcon from '@lucide/svelte/icons/package-open'
import PenIcon from '@lucide/svelte/icons/pen'
import PenLineIcon from '@lucide/svelte/icons/pen-line'
import PlugIcon from '@lucide/svelte/icons/plug'
import SearchIcon from '@lucide/svelte/icons/search'
import SendIcon from '@lucide/svelte/icons/send'
import SquareDashedMousePointerIcon from '@lucide/svelte/icons/square-dashed-mouse-pointer'
import WrenchIcon from '@lucide/svelte/icons/wrench'
import ZapIcon from '@lucide/svelte/icons/zap'
import type { IconComponent } from '$lib/layout/types'

const BY_NAME: Record<string, IconComponent> = {
  Bash: ChevronRightIcon,
  bash: ChevronRightIcon,
  Shell: ChevronRightIcon,
  PowerShell: ChevronRightIcon,
  Read: BookOpenCheckIcon,
  Grep: SearchIcon,
  Search: SearchIcon,
  CodeSearch: SearchIcon,
  CodeScout: CodeIcon,
  CodeStats: ChartLineIcon,
  Job: CpuIcon,
  Write: PenIcon,
  Edit: PenLineIcon,
  MultiEdit: PenLineIcon,
  NotebookEdit: PenLineIcon,
  ApplyPatch: FileDiffIcon,
  Glob: FolderSearchIcon,
  ViewImage: ImageIcon,
  WebFetch: GlobeIcon,
  WebSearch: BinocularsIcon,
  TodoWrite: ListTodoIcon,
  Ask: MessageCircleQuestionIcon,
  Agent: BotIcon,
  Agents: BotIcon,
  AgentControl: BotIcon,
  AgentProfiles: BotIcon,
  SendMessage: SendIcon,
  LoadTools: PackageOpenIcon,
  Skill: ZapIcon,
  LSP: BracesIcon,
  memory_search: DatabaseIcon,
  memory_write: DatabaseIcon,
}

/** The step of a run that is reasoning, not calling a tool. */
export const THINKING_ICON: IconComponent = BrainIcon
/** The step of a run that is writing a message. */
export const MESSAGE_ICON: IconComponent = MessageSquareIcon

export function toolIcon(name: string): IconComponent {
  const known = BY_NAME[name]
  if (known) return known
  if (name.startsWith('mcp__computer-use__')) return SquareDashedMousePointerIcon
  if (name.startsWith('mcp__')) return PlugIcon
  return WrenchIcon
}
