// The RustGIS pane's input layer: a transparent region over the screen that turns the person's
// keys and pointer into rustgis-tui's input events and posts them, batched, to the hooks module.
import type { ClientKeyEvent, ClientModule } from 'claude-code'

type State = { queue: unknown[] }

const NAMES: Record<string, string> = {
  up: 'Up',
  down: 'Down',
  left: 'Left',
  right: 'Right',
  return: 'Enter',
  backspace: 'Backspace',
  delete: 'Delete',
  pageup: 'PageUp',
  pagedown: 'PageDown',
  home: 'Home',
  end: 'End',
  ' ': 'Space',
}

// A key as rustgis-tui's key map spells it ("j", "S-Left", "C-c", "BackTab").
function spell(k: ClientKeyEvent): string | null {
  if (k.key === 'tab') return k.shift ? 'BackTab' : 'Tab'
  const name = NAMES[k.key]
  if (name) return k.shift && name !== 'Space' ? `S-${name}` : name
  if ([...k.key].length !== 1) return null
  return k.ctrl ? `C-${k.key}` : k.key
}

const Input: ClientModule<null, State> = (_props, surface) => {
  if (surface.state === undefined) {
    const queue: unknown[] = []
    let at = ''
    surface.setState({ queue })
    surface.onKey(k => {
      const key = spell(k)
      if (key) queue.push({ key })
    })
    surface.onPointer(p => {
      if (p.type === 'enter' || p.type === 'leave') return
      if (p.type === 'move' && `${p.x},${p.y},${p.button}` === at) return // same cell
      at = `${p.x},${p.y},${p.button}`
      queue.push({ mouse: p.type, x: p.x, y: p.y, button: p.button, shift: p.shift })
    })
    surface.every(20, () => {
      if (queue.length) surface.post({ events: queue.splice(0) as never })
    })
  }
  return surface.elements.Box({ width: surface.columns, height: surface.rows })
}

export default Input
