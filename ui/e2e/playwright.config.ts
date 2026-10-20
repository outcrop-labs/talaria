// The MCP end-to-end suite (docs/E2E.md). One browser, one worker, the tests
// in file order — but not `serial`: a red chapter must not hide the ones
// after it, so each chapter rebuilds what it needs (mcp.spec.ts). Everything a run writes — traces, screenshots, the minted
// session — lands under E2E_OUTPUT_DIR / E2E_REPORT_DIR / E2E_STATE_DIR
// (default: beside this file, gitignored). Keep anything else a run writes
// (a json report, logs) OUT of E2E_OUTPUT_DIR — it is wiped at start.
import { defineConfig, devices } from '@playwright/test'
import { join } from 'node:path'
import { BASE_URL, STATE_FILE } from './support'

const here = import.meta.dirname
const outputDir = process.env.E2E_OUTPUT_DIR || join(here, 'test-results')
// BESIDE outputDir, never inside it: Playwright empties outputDir when a run
// starts, and refuses (loudly, every run) an html report that lives in it.
const reportDir = process.env.E2E_REPORT_DIR || join(here, 'report')

export default defineConfig({
  testDir: here,
  testMatch: '*.spec.ts',
  fullyParallel: false,
  workers: 1,
  // No retries: a chapter that passes on its second try passed against state
  // its first try left behind, which is not the claim the suite makes.
  retries: 0,
  forbidOnly: !!process.env.CI,
  timeout: 180_000,
  expect: { timeout: 30_000 },
  globalSetup: join(here, 'global-setup.ts'),
  outputDir,
  reporter: process.env.CI
    ? [['list'], ['github'], ['html', { open: 'never', outputFolder: reportDir }]]
    : [['list']],
  use: {
    baseURL: BASE_URL,
    storageState: STATE_FILE,
    // The stub's OAuth pages are served by a throwaway CA; the browser only
    // passes through them on the way back to Talaria's callback.
    ignoreHTTPSErrors: true,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    viewport: { width: 1440, height: 960 },
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 960 } } }],
})
