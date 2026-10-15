/** The number of the frame the pane last drew in full (it redraws when this changes). */
export type FrameNumber = number

declare module 'claude-code' {
  interface PluginState {
    'rustgis-tui': { gen: FrameNumber }
  }
}
