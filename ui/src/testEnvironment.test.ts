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
})
