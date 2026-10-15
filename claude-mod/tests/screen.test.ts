import { expect, test } from 'claude-code/testing'

test('the screen tool says how to start RustGIS when it is not running', async $ => {
  const r = await $.tool.call({ tool: 'mcp__rustgis-tui__screen', input: {} })
  expect(JSON.stringify(r)).toContain('not running')
})

