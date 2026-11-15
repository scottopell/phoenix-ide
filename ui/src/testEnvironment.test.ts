import { afterEach, describe, expect, it } from 'vitest'

describe('browser test environment', () => {
  afterEach(() => {
    localStorage.clear()
    sessionStorage.clear()
  })

  it('uses Happy DOM storage instead of Node worker storage', () => {
    localStorage.setItem('phoenix-test', 'local')
    sessionStorage.setItem('phoenix-test', 'session')

    expect(localStorage.getItem('phoenix-test')).toBe('local')
    expect(sessionStorage.getItem('phoenix-test')).toBe('session')
  })

  it('keeps iframe page loading disabled', () => {
    const happyDOM = (window as unknown as {
      happyDOM: { settings: { disableIframePageLoading: boolean } }
    }).happyDOM

    expect(happyDOM.settings.disableIframePageLoading).toBe(true)
  })

  // happy-dom <20.11.2 held the observer callback only through a WeakRef, so a
  // garbage collection silently stopped delivery and DOM-driven UI never updated.
  it('keeps delivering MutationObserver records after garbage collection', async () => {
    const gc = (globalThis as { gc?: () => void }).gc
    expect(gc, 'vitest workers must run with --expose-gc').toBeTypeOf('function')
    const host = document.createElement('div')
    document.body.append(host)
    let delivered = 0
    new MutationObserver(() => { delivered += 1 }).observe(document.body, { childList: true, subtree: true })
    gc!()
    await new Promise(resolve => setTimeout(resolve, 0))
    gc!()
    host.append(document.createElement('span'))
    await new Promise(resolve => setTimeout(resolve, 0))
    expect(delivered).toBe(1)
    host.remove()
  })
})
