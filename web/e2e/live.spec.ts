import { type Browser, type Page, expect, test } from '@playwright/test'

const CODE = process.env.ADMIN_CODE ?? 'school1448'

async function adminPage(browser: Browser): Promise<Page> {
  const ctx = await browser.newContext({ permissions: ['camera', 'microphone'] })
  const page = await ctx.newPage()
  const res = await page.request.post('/api/admin/login', { data: { code: CODE } })
  expect(res.ok()).toBeTruthy()
  return page
}

async function viewerStats(page: Page) {
  return page.evaluate(() => (window as any).__viewerStats as { firstFrameMs: number | null; firstFramePageMs: number | null; height: number; fps: number; bitrate: number } | undefined)
}

test('phone publishes, viewers watch with low latency, recording lands in the archive', async ({ browser }) => {
  const studio = await adminPage(browser)
  const created = await studio.request.post('/api/admin/streams', {
    data: { title: `E2E эфир ${Date.now()}`, category: 'concert', description: 'Автотест', scheduledAt: null },
  })
  expect(created.status()).toBe(201)
  const stream = await created.json()

  // --- Phone studio goes live ---
  await studio.goto(`/studio/${stream.id}`)
  await studio.getByRole('button', { name: 'Начать эфир' }).click()
  await expect(studio.locator('.badge-live')).toBeVisible({ timeout: 15_000 })

  // Viewers normally join a running broadcast: wait until the phone's video arrives.
  await expect
    .poll(async () => {
      const st = await (await studio.request.get('/api/admin/status')).json()
      return st.rooms.find((r: any) => r.streamId === stream.id)?.layers.length ?? 0
    }, { timeout: 15_000 })
    .toBeGreaterThanOrEqual(2)

  // --- Viewer opens the stream page ---
  const viewerCtx = await browser.newContext()
  const viewer = await viewerCtx.newPage()
  const t0 = Date.now()
  await viewer.goto(`/live/${stream.id}`)
  await expect.poll(async () => (await viewerStats(viewer))?.firstFrameMs ?? null, { timeout: 15_000 }).not.toBeNull()
  const wallToFirstFrame = Date.now() - t0
  const first = await viewerStats(viewer)
  console.log(`time to first frame: ${first?.firstFrameMs} ms after player start, ${first?.firstFramePageMs} ms after navigation start (test wall clock ${wallToFirstFrame} ms)`)
  expect(first!.firstFrameMs!).toBeLessThan(1500)

  // The home page shows the same broadcast in its hero player.
  const homeCtx = await browser.newContext()
  const home = await homeCtx.newPage()
  await home.goto('/')
  await expect.poll(async () => (await viewerStats(home))?.firstFrameMs ?? null, { timeout: 10_000 }).not.toBeNull()
  await homeCtx.close()

  // Simulcast: after a few seconds the viewer climbs to the top layer.
  await expect.poll(async () => (await viewerStats(viewer))?.height ?? 0, { timeout: 20_000 }).toBeGreaterThanOrEqual(700)
  const top = await viewerStats(viewer)
  console.log(`top layer: ${top?.height}p @ ${top?.fps} fps, ${Math.round((top?.bitrate ?? 0) / 1000)} kbit/s`)

  // Manual quality: "Экономия трафика" switches to the low layer.
  await viewer.locator('.player').hover()
  await viewer.locator('.quality-btn').click()
  await viewer.getByRole('menuitemradio', { name: 'Экономия трафика' }).click()
  await expect.poll(async () => (await viewerStats(viewer))?.height ?? 9999, { timeout: 10_000 }).toBeLessThan(400)
  console.log(`low layer: ${(await viewerStats(viewer))?.height}p`)

  // The admin panel sees the viewer.
  await expect
    .poll(async () => {
      const s = await (await studio.request.get('/api/admin/status')).json()
      return s.rooms.find((r: any) => r.streamId === stream.id)?.viewers ?? 0
    }, { timeout: 10_000 })
    .toBeGreaterThanOrEqual(1)

  // --- Takeover from a second device: viewers keep playing ---
  const phone2 = await adminPage(browser)
  await phone2.goto(`/studio/${stream.id}`)
  await phone2.getByRole('button', { name: 'Начать эфир' }).click()
  await phone2.getByRole('button', { name: 'Перехватить эфир на это устройство' }).click()
  await expect(phone2.locator('.badge-live')).toBeVisible({ timeout: 15_000 })
  const framesBefore = await viewer.evaluate(() => (document.querySelector('video') as HTMLVideoElement).getVideoPlaybackQuality().totalVideoFrames)
  await viewer.waitForTimeout(4000)
  const framesAfter = await viewer.evaluate(() => (document.querySelector('video') as HTMLVideoElement).getVideoPlaybackQuality().totalVideoFrames)
  console.log(`frames during takeover window: ${framesAfter - framesBefore}`)
  expect(framesAfter - framesBefore).toBeGreaterThan(30)
  await studio.close()

  // Let the recording run a bit longer, then end the broadcast from the studio.
  await phone2.waitForTimeout(6000)
  await phone2.getByRole('button', { name: 'Завершить эфир' }).click()
  await phone2.getByRole('button', { name: 'Завершить', exact: true }).click()
  await expect(phone2.getByText('Эфир завершён')).toBeVisible({ timeout: 10_000 })

  // --- Recording is processed and plays from the archive ---
  let recording: any = null
  await expect
    .poll(async () => {
      const page = await (await phone2.request.get('/api/admin/recordings')).json()
      recording = page.items.find((r: any) => r.streamId === stream.id)
      return recording?.status
    }, { timeout: 90_000, intervals: [1000] })
    .toBe('ready')
  console.log(`recording ready: ${recording.durationMs} ms, ${recording.width}x${recording.height}, qualities ${recording.qualities}`)
  expect(recording.durationMs).toBeGreaterThan(8000)

  await viewer.goto(`/archive/${recording.id}`)
  await viewer.locator('video').evaluate((v: HTMLVideoElement) => {
    v.muted = true
    return v.play()
  })
  await expect.poll(() => viewer.locator('video').evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(1)
})
