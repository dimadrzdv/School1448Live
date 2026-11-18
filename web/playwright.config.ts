import { defineConfig } from '@playwright/test'

// Runs against `scripts/dev-server.sh` (:8080) + `npm run dev` (:5173).
export default defineConfig({
  testDir: './e2e',
  timeout: 180_000,
  fullyParallel: false,
  workers: 1,
  reporter: [['list']],
  use: {
    baseURL: process.env.BASE_URL ?? 'http://localhost:5173',
    // scripts/phone-test.sh uses a local CA the test browser does not know.
    ignoreHTTPSErrors: process.env.BASE_URL?.startsWith('https://') ?? false,
    channel: 'chrome',
    headless: true,
    permissions: ['camera', 'microphone'],
    launchOptions: {
      args: ['--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream', '--autoplay-policy=no-user-gesture-required'],
    },
  },
})
