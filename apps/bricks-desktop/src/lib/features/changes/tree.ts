// Changed files as a folder tree. Folders come first, then files, each in name
// order. A folder with a single folder inside is shown as one row (« src/lib »),
// as editors do, so deep paths stay short.

export type TreeNode<T> =
	| { type: 'folder'; name: string; path: string; children: TreeNode<T>[] }
	| { type: 'file'; name: string; path: string; item: T }

interface Folder<T> {
	folders: Map<string, Folder<T>>
	files: { name: string; path: string; item: T }[]
}

export function buildFileTree<T>(items: readonly T[], pathOf: (item: T) => string): TreeNode<T>[] {
	const root: Folder<T> = { folders: new Map(), files: [] }
	for (const item of items) {
		const path = pathOf(item)
		const parts = path.split('/').filter((part) => part !== '')
		const name = parts.pop() ?? path
		let folder = root
		for (const part of parts) {
			let next = folder.folders.get(part)
			if (!next) {
				next = { folders: new Map(), files: [] }
				folder.folders.set(part, next)
			}
			folder = next
		}
		folder.files.push({ name, path, item })
	}
	return nodesOf(root, '')
}

function nodesOf<T>(folder: Folder<T>, prefix: string): TreeNode<T>[] {
	const byName = (a: { name: string }, b: { name: string }) => a.name.localeCompare(b.name)
	const folders: TreeNode<T>[] = [...folder.folders.entries()]
		.map(([name, inner]) => {
			// Merge a chain of single-folder folders into one row.
			let label = name
			let current = inner
			while (current.files.length === 0 && current.folders.size === 1) {
				const [childName, child] = [...current.folders.entries()][0]
				label = `${label}/${childName}`
				current = child
			}
			const path = prefix ? `${prefix}/${label}` : label
			return { type: 'folder' as const, name: label, path, children: nodesOf(current, path) }
		})
		.sort(byName)
	const files: TreeNode<T>[] = folder.files.map((file) => ({ type: 'file' as const, ...file })).sort(byName)
	return [...folders, ...files]
}
