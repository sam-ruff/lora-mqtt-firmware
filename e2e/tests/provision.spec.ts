import { expect, test } from '@playwright/test';

// Provisions the hub through the portal UI. Destructive: on success the hub
// reboots onto the configured network and the hotspot disappears, so this
// only runs when explicitly requested (PROVISION=1) and after portal.spec.

const provision = process.env.PROVISION === '1';
const wifiSsid = process.env.HUB_WIFI_SSID ?? '';
const wifiPassword = process.env.HUB_WIFI_PASSWORD ?? '';
const brokerHost = process.env.HUB_MQTT_HOST ?? '';

test.skip(!provision, 'set PROVISION=1 (and the HUB_* env vars) to provision the hub');

test('provision the hub through the portal form', async ({ page }) => {
  expect(wifiSsid, 'HUB_WIFI_SSID must be set').not.toBe('');
  expect(brokerHost, 'HUB_MQTT_HOST must be set').not.toBe('');

  await page.goto('/');
  // The page prefills from /api/config; wait for that fetch to land.
  await page.waitForLoadState('networkidle');

  await page.locator('input[name="wifi_ssid"]').fill(wifiSsid);
  await page.locator('input[name="wifi_password"]').fill(wifiPassword);
  await page.locator('#mode').selectOption('bridge');
  await page.locator('input[name="mqtt_host"]').fill(brokerHost);
  await page.locator('input[name="mqtt_port"]').fill('1883');

  await page.getByRole('button', { name: 'Save and restart' }).click();

  // The hub answers before scheduling its reboot; either the saved message
  // or the no-reply fallback (response lost to the reboot) is acceptable.
  await expect(page.locator('#msg')).toContainText(/Saved|restarting/, { timeout: 20_000 });
});
