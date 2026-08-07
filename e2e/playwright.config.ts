import { defineConfig, devices } from '@playwright/test';

// The portal runs on the hub's SoftAP; run_e2e.sh joins that network first.
const portalUrl = process.env.HUB_PORTAL_URL ?? 'http://192.168.4.1';

export default defineConfig({
  testDir: './tests',
  timeout: 60_000,
  expect: { timeout: 15_000 },
  // One worker: the tests drive one physical hub through state transitions.
  workers: 1,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: portalUrl,
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'desktop',
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'mobile',
      use: { ...devices['Pixel 7'] },
    },
  ],
});
