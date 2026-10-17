// Pure helpers shared by the composer components. No Svelte runes here.
// Owner: Haiku 5 (Sprint 14, composer).

/** Extensions sent as `image` blocks; the engine decides what the model accepts. */
const IMAGE_EXTENSIONS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp'])

/** Last path segment, without a trailing separator. */
export function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '')
  const i = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  return i >= 0 ? trimmed.slice(i + 1) : trimmed
}

/** True when the path looks like an image file (by extension only). */
export function isImagePath(path: string): boolean {
  const dot = path.lastIndexOf('.')
  if (dot < 0) return false
  return IMAGE_EXTENSIONS.has(path.slice(dot + 1).toLowerCase())
}

/** File extension for a media type, e.g. `image/png` gives `png`. */
export function extensionFor(mediaType: string): string {
  const sub = (mediaType.split('/')[1] ?? '').split('+')[0] ?? ''
  const ext = sub.replace(/[^a-z0-9]/gi, '').toLowerCase()
  return ext || 'png'
}

/** Base64 payload of a blob (the data URL prefix is removed). */
export function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const result = String(reader.result ?? '')
      resolve(result.slice(result.indexOf(',') + 1))
    }
    reader.onerror = () => reject(reader.error ?? new Error('Lecture de la pièce jointe impossible.'))
    reader.readAsDataURL(blob)
  })
}
