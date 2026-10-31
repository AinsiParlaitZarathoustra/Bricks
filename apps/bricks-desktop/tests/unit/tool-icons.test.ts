// The icon of each tool, by its real name (timeline/toolIcons.ts). The icons are compared with
// the components of @lucide/svelte by identity, so a wrong mapping fails here, not by eye.
import { describe, expect, it } from 'vitest';
import BinocularsIcon from '@lucide/svelte/icons/binoculars';
import BookOpenCheckIcon from '@lucide/svelte/icons/book-open-check';
import BotIcon from '@lucide/svelte/icons/bot';
import BrainIcon from '@lucide/svelte/icons/brain';
import ChartLineIcon from '@lucide/svelte/icons/chart-line';
import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
import CodeIcon from '@lucide/svelte/icons/code';
import CpuIcon from '@lucide/svelte/icons/cpu';
import MessageSquareIcon from '@lucide/svelte/icons/message-square';
import PenIcon from '@lucide/svelte/icons/pen';
import PenLineIcon from '@lucide/svelte/icons/pen-line';
import PlugIcon from '@lucide/svelte/icons/plug';
import SearchIcon from '@lucide/svelte/icons/search';
import SquareDashedMousePointerIcon from '@lucide/svelte/icons/square-dashed-mouse-pointer';
import WrenchIcon from '@lucide/svelte/icons/wrench';
import {
	MESSAGE_ICON,
	THINKING_ICON,
	toolIcon,
} from '../../src/lib/features/conversation/timeline/toolIcons';

describe('toolIcon: the icon says what the tool does', () => {
	it.each([
		['Bash', ChevronRightIcon],
		['Read', BookOpenCheckIcon],
		['Grep', SearchIcon],
		['CodeSearch', SearchIcon],
		['CodeScout', CodeIcon],
		['CodeStats', ChartLineIcon],
		['Job', CpuIcon],
		['Write', PenIcon],
		['Edit', PenLineIcon],
		['WebSearch', BinocularsIcon],
		['Agent', BotIcon],
	])('%s has its own icon', (name, icon) => {
		expect(toolIcon(name)).toBe(icon);
	});

	it('the shell tools share the chevron, whatever their spelling', () => {
		expect(toolIcon('bash')).toBe(ChevronRightIcon);
		expect(toolIcon('PowerShell')).toBe(ChevronRightIcon);
	});

	it('a computer-use tool shows the pointer, whatever its action', () => {
		expect(toolIcon('mcp__computer-use__screenshot')).toBe(SquareDashedMousePointerIcon);
		expect(toolIcon('mcp__computer-use__left_click')).toBe(SquareDashedMousePointerIcon);
	});

	it('any other MCP tool shows the plug', () => {
		expect(toolIcon('mcp__autre__x')).toBe(PlugIcon);
		expect(toolIcon('mcp__github__create_issue')).toBe(PlugIcon);
	});

	it('an unknown tool shows the wrench', () => {
		expect(toolIcon('InconnuDuTout')).toBe(WrenchIcon);
		expect(toolIcon('')).toBe(WrenchIcon);
	});

	it('the step icons of a run are the brain for reasoning and the message square for a message', () => {
		expect(THINKING_ICON).toBe(BrainIcon);
		expect(MESSAGE_ICON).toBe(MessageSquareIcon);
	});
});
