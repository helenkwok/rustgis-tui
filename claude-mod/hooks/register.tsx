// RustGIS in a pane. rustgis-tui runs in serve mode (`--serve DIR`, no terminal of its own): it
// writes each frame to DIR (screen cells for a Raster, plain text, the map PNG and where it sits)
// and reads input files from DIR/in. This pane shows the frame with the real map laid over it,
// and sends the person's keys and clicks; the `screen` and `send` tools let Claude drive the same
// running app. One app, one input queue, so the two never disagree.
import { atom, read, update } from 'claude-code'
import type { Register } from 'claude-code'

const PANE = 'rustgis'
// One folder per loaded copy of the mod, so two sessions never share one; removed when it stops.
const DIR = `/tmp/rustgis-tui-pane-${Math.random().toString(36).slice(2, 10)}`
const shown = atom({ plugin: 'rustgis-tui', key: 'gen' } as const, 0)

type MapRect = { x: number; y: number; w: number; h: number; file: string }
type Frame = { gen: number; cols: number; rows: number; cells: string; text: string[]; map: MapRect | null }

let frame: Frame | null = null
let running = false
let seq = 0
let mounted = '' // the layout the pane last drew; a frame with the same one is blitted
let asked = '' // the size last asked of the app
let pointer: { x: number; y: number } | null = null
let lively = 0 // when input or a new frame last came: frames are checked often for 2 s after

const layout = (f: Frame) => `${f.cols}x${f.rows}:${f.map ? [f.map.x, f.map.y, f.map.w, f.map.h].join(',') : '-'}`

async function send($: any, events: unknown[]) {
  seq += 1
  lively = Date.now()
  const name = `${String(Date.now()).padStart(15, '0')}-${String(seq).padStart(6, '0')}.json`
  await $.fs.write(`${DIR}/in/${name}`, JSON.stringify(events))
}

// Start rustgis-tui once, and watch its frames for as long as it runs.
async function start($: any) {
  if (running) return
  running = true
  frame = null
  mounted = ''
  asked = ''
  const found = await $.process.run(['/bin/sh', '-lc', 'command -v rustgis-tui; command -v rustgis-cli'])
  const [tui, cli] = found.stdout.trim().split('\n')
  if (!tui || !cli) {
    running = false
    $.ui.toast('rustgis-tui or rustgis-cli is not on PATH')
    return
  }
  await $.process.run(['/bin/rm', '-rf', DIR])
  void (async () => {
    const app = $.process.spawn({ argv: [tui, '--serve', DIR], env: { RUSTGIS_CLI: cli } })
    for await (const piece of app) {
      if (piece.stream === 'stderr') $.ui.log(`rustgis-tui: ${piece.text}`, { to: 'debug' })
    }
    running = false
    await $.process.run(['/bin/rm', '-rf', DIR])
  })()
  void (async () => {
    let last = ''
    while (running) {
      await $.clock.sleep(Date.now() - lively < 2000 ? 40 : 250)
      let gen = ''
      try {
        gen = await $.fs.read(`${DIR}/gen`)
      } catch {
        continue // not started yet
      }
      if (gen === last) continue
      last = gen
      lively = Date.now()
      let f: Frame
      try {
        f = JSON.parse(await $.fs.read(`${DIR}/frame.json`))
      } catch {
        continue
      }
      const before = frame
      frame = f
      // Same layout: swap the cells and the picture in place. Otherwise draw the pane again.
      let redraw = layout(f) !== mounted
      if (!redraw) {
        const cells = await $.ui.blit({ requestId: PANE, key: 'screen', cells: f.cells })
        if ('deny' in cells) redraw = true
        if (!redraw && f.map && f.map.file !== before?.map?.file) {
          const pic = await $.ui.blit({ requestId: PANE, key: 'map', source: { file: f.map.file, format: 'png' } })
          if ('deny' in pic) redraw = true
        }
      }
      if (redraw) await update($, shown, () => f.gen)
    }
    frame = null
    mounted = ''
    await update($, shown, n => n + 1)
  })()
}

async function stop($: any) {
  if (running) await send($, [{ quit: true }])
  running = false
}

// Wait (up to `ms`) for a frame newer than `gen`.
async function settle($: any, gen: number, ms: number) {
  for (let t = 0; t < ms && (frame?.gen ?? 0) <= gen; t += 50) await $.clock.sleep(50)
}

function screenText() {
  if (!frame) return 'RustGIS is not running. Call send (or /rustgis) to start it.'
  const m = frame.map
  const where = m
    ? `The map picture is ${m.file}, drawn over columns ${m.x}-${m.x + m.w - 1}, rows ${m.y}-${m.y + m.h - 1}; those cells are blank in the text.`
    : 'No map picture.'
  return `Frame ${frame.gen}, ${frame.cols}x${frame.rows} cells (x = column, y = row, from 0).\n${where}\n\n${frame.text.join('\n')}`
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: 'rustgis',
      description: 'Open RustGIS in a pane you and Claude both drive (/rustgis close stops it)',
      argumentHint: '[close]',
    })
    await $.tool.register({
      name: 'screen',
      description:
        "Read the RustGIS pane: rustgis-tui's whole screen as text, and the path of the map picture shown over the map area. The person sees and drives the same app.",
      inputSchema: { type: 'object', properties: {} },
    })
    await $.tool.register({
      name: 'send',
      description:
        "Drive RustGIS in the pane (the same running app the person sees and drives), then read the screen. Keys use rustgis-tui's own spelling: a character (\"j\", \"U\", \":\"), or Enter, Esc, Tab, BackTab, Backspace, Space, Up/Down/Left/Right, Home, End, PageUp, PageDown, with S- or C- for Shift or Ctrl (\"S-Left\"). x is the column and y the row on screen, from 0, as in the screen text. Starts RustGIS if it is not running.",
      inputSchema: {
        type: 'object',
        properties: {
          keys: { type: 'array', items: { type: 'string' }, description: 'Keys, in order' },
          text: { type: 'string', description: 'Characters to type, one key each (palette, prompts)' },
          click: {
            type: 'object',
            properties: {
              x: { type: 'integer' },
              y: { type: 'integer' },
              button: { type: 'string', enum: ['left', 'right'] },
              shift: { type: 'boolean' },
              double: { type: 'boolean' },
            },
            required: ['x', 'y'],
          },
          drag: {
            type: 'object',
            properties: {
              from: { type: 'array', items: { type: 'integer' }, minItems: 2, maxItems: 2 },
              to: { type: 'array', items: { type: 'integer' }, minItems: 2, maxItems: 2 },
              button: { type: 'string', enum: ['left', 'right'] },
            },
            required: ['from', 'to'],
          },
          wheel: {
            type: 'object',
            properties: { x: { type: 'integer' }, y: { type: 'integer' }, dir: { type: 'string', enum: ['up', 'down'] }, times: { type: 'integer' } },
            required: ['x', 'y', 'dir'],
          },
        },
      },
    })
    return next(e)
  })

  on('command.run', { command: 'rustgis' }, async ($, e) => {
    if (String(e.args ?? '').trim() === 'close') {
      await stop($)
      await $.ui.close({ id: PANE })
      return { text: 'RustGIS stopped.' }
    }
    await start($)
    await $.ui.open({ id: PANE, title: 'RustGIS', focus: true })
    return { text: 'RustGIS is open. Click the pane to type into it; ` stands in for Escape there.' }
  })

  on('ui.close', { id: PANE }, async ($, e, next) => {
    await stop($)
    return next(e)
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    await read($, shown)
    if (e.surface !== 'terminal') {
      const { Text } = $.ui.resolve(e)
      return <Text>RustGIS draws in the terminal.</Text>
    }
    const { Box, Text, Raster, Image, Client } = $.ui.resolve(e)
    // Ask the app for the pane's size; it draws the next frame to fit.
    const size = `${e.props.bodyColumns}x${e.props.scroll.bodyRows}`
    if (running && size !== asked && e.props.bodyColumns > 0) {
      asked = size
      void send($, [{ size: [e.props.bodyColumns, e.props.scroll.bodyRows] }])
    }
    if (!frame) return <Text dimColor>{running ? 'Starting RustGIS…' : 'RustGIS is not running: /rustgis starts it.'}</Text>
    const f = frame
    mounted = layout(f)
    return (
      <Box position="relative" width={f.cols} height={f.rows}>
        <Raster key="screen" columns={f.cols} rows={f.rows} cells={f.cells} />
        {f.map && (
          <Box position="absolute" top={f.map.y} left={f.map.x}>
            <Image key="map" source={{ file: f.map.file, format: 'png' }} columns={f.map.w} rows={f.map.h} alt="map" />
          </Box>
        )}
        <Box position="absolute" top={0} left={0}>
          <Client key="input" module="./surface.tsx" width={f.cols} height={f.rows} />
        </Box>
      </Box>
    )
  })

  // The person's keys and pointer, batched by the input layer (surface.tsx).
  on('ui.message', { requestId: PANE }, async ($, e) => {
    const events = (e.data as { events?: { mouse?: string; x?: number; y?: number }[] })?.events ?? []
    for (const ev of events) if (ev.mouse && ev.x !== undefined && ev.y !== undefined) pointer = { x: ev.x, y: ev.y }
    if (events.length && running) await send($, events)
    return {}
  })

  // The wheel arrives as a scroll of the pane: hand it to the app where the pointer is.
  on('ui.scroll', { requestId: PANE }, async ($, e, next) => {
    if (running && pointer && e.by !== 0) {
      const n = Math.min(10, Math.abs(Math.round(e.by)) || 1)
      await send($, Array.from({ length: n }, () => ({ mouse: 'scroll', dir: e.by < 0 ? 'up' : 'down', ...pointer })))
    }
    return next(e)
  })

  on('tool.call', { tool: 'mcp__rustgis-tui__screen' }, async () => ({ result: screenText() }))

  on('tool.call', { tool: 'mcp__rustgis-tui__send' }, async ($, e) => {
    // the tool's own arguments sit on the event beside `tool`
    const input = e as unknown as {
      keys?: string[]
      text?: string
      click?: { x: number; y: number; button?: string; shift?: boolean; double?: boolean }
      drag?: { from: number[]; to: number[]; button?: string }
      wheel?: { x: number; y: number; dir: string; times?: number }
    }
    if (!running) await start($)
    for (let t = 0; t < 8000 && !frame; t += 100) await $.clock.sleep(100)
    const gen = frame?.gen ?? 0
    const events: unknown[] = []
    for (const k of input.keys ?? []) events.push({ key: k })
    for (const ch of Array.from(input.text ?? '')) events.push({ key: ch === ' ' ? 'Space' : ch })
    if (input.click) {
      const { x, y, button = 'left', shift } = input.click
      for (let i = 0; i < (input.click.double ? 2 : 1); i++) {
        events.push({ mouse: 'down', x, y, button, shift }, { mouse: 'up', x, y, button, shift })
      }
    }
    if (input.drag) {
      const [x0, y0] = input.drag.from
      const [x1, y1] = input.drag.to
      const button = input.drag.button ?? 'left'
      events.push({ mouse: 'down', x: x0, y: y0, button }, { mouse: 'move', x: x1, y: y1, button }, { mouse: 'up', x: x1, y: y1, button })
    }
    if (input.wheel) {
      const { x, y, dir, times = 1 } = input.wheel
      for (let i = 0; i < Math.min(times, 20); i++) events.push({ mouse: 'scroll', x, y, dir })
    }
    if (events.length) await send($, events)
    await settle($, gen, 3000)
    await $.clock.sleep(150) // a command's redraw can take a second frame
    return { result: screenText() }
  }).catch(($, e, next) => ({ result: `RustGIS: the send failed (${JSON.stringify(next.error)}).

${screenText()}` }))
}
