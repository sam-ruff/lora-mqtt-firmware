import { expect, test } from '@playwright/test';

// Read-only portal checks; safe to run repeatedly and on every viewport.

test('config page loads with the setup form', async ({ page }) => {
  await page.goto('/');
  await expect(page).toHaveTitle('Walkie-Textie Hub');
  await expect(page.getByRole('heading', { name: 'Walkie-Textie Hub setup' })).toBeVisible();
  await expect(page.locator('input[name="wifi_ssid"]')).toBeVisible();
  await expect(page.locator('input[name="wifi_password"]')).toBeVisible();
  await expect(page.locator('input[name="mqtt_host"]')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save and restart' })).toBeVisible();
});

test('page fits the viewport with no horizontal scroll', async ({ page }) => {
  await page.goto('/');
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth,
  );
  expect(overflow).toBeLessThanOrEqual(0);
});

test('gateway settings appear only in gateway mode', async ({ page }) => {
  await page.goto('/');
  const gatewayBox = page.locator('#gwbox');
  await expect(gatewayBox).toBeHidden();
  await page.locator('#mode').selectOption('gateway');
  await expect(gatewayBox).toBeVisible();
  await page.locator('#mode').selectOption('bridge');
  await expect(gatewayBox).toBeHidden();
});

test('config API returns the expected shape without a password', async ({ request }) => {
  const response = await request.get('/api/config');
  expect(response.status()).toBe(200);
  const config = await response.json();
  expect(config).toHaveProperty('mode');
  expect(config).toHaveProperty('wifi_ssid');
  expect(config).toHaveProperty('mqtt_host');
  expect(config).toHaveProperty('mqtt_port');
  expect(config).toHaveProperty('gw_freq_hz');
  expect(config).not.toHaveProperty('wifi_password');
});

test('status API reports link states and counters', async ({ request }) => {
  const response = await request.get('/api/status');
  expect(response.status()).toBe(200);
  const status = await response.json();
  expect(status).toHaveProperty('wifi_state');
  expect(status).toHaveProperty('mqtt_state');
  expect(status).toHaveProperty('ip');
  expect(status).toHaveProperty('uptime_secs');
});

test('invalid config posts are rejected', async ({ request }) => {
  const bad = await request.post('/api/config', {
    data: { gw_sf: 42, wifi_ssid: 'x' },
  });
  expect(bad.status()).toBe(400);
  const body = await bad.json();
  expect(body.ok).toBe(false);

  const empty = await request.post('/api/config', { data: {} });
  expect(empty.status()).toBe(400);
});

test('unknown paths redirect to the portal (captive portal)', async ({ request }) => {
  const response = await request.get('/generate_204', { maxRedirects: 0 });
  expect(response.status()).toBe(302);
  expect(response.headers()['location']).toBe('http://192.168.4.1/');
});
