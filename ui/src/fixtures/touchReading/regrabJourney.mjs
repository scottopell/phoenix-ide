export async function runRegrabJourney({ page, dispatch, pause, now }) {
  const commands = [];
  const origin = now();
  const send = async (type, x, y) => {
    const sentAt = now() - origin;
    await dispatch(type, x, y);
    commands.push({ type, sentAt, acknowledgedAt: now() - origin, x, y });
  };
  try {
    const first = await page.evaluate(() => window.__regrabProbe.firstContact());
    await send('touchStart', first.x, first.y);
    const flickStart = now();
    for (let i = 1; i <= 5; i++) {
      await pause(Math.max(0, flickStart + i * 16 - now()));
      await send('touchMove', first.x, first.y + i * 32);
    }
    await send('touchEnd', first.x, first.y + 160);
    await pause(240);
    const second = await page.evaluate(() => window.__regrabProbe.prepare());
    await send('touchStart', second.x, second.y);
    await pause(2600);
    await send('touchEnd', second.x, second.y);
    await pause(350);
    const trace = await page.evaluate(() => window.__regrabProbe.stop());
    return { trace, commands, qualification: 'NOT_QUALIFIED', causalAttribution: 'UNKNOWN',
      error: trace.status === 'OBSERVED_UNATTRIBUTED' ? null : 'Regrab continuity unobservable' };
  } catch {
    const trace = await page.evaluate(() => window.__regrabProbe?.stop() ?? null).catch(() => null);
    return { trace, commands, qualification: 'NOT_QUALIFIED', causalAttribution: 'UNKNOWN',
      error: 'Regrab acquisition or observation failed' };
  }
}
