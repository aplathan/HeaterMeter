// Tiny history-API router.

export const route = $state({ path: location.pathname })

export function navigate(path: string) {
  if (path === route.path) return
  history.pushState({}, '', path)
  route.path = path
  window.scrollTo({ top: 0 })
}

window.addEventListener('popstate', () => {
  route.path = location.pathname
})

/** Intercept clicks on same-origin <a> links. */
export function link(node: HTMLAnchorElement) {
  const onClick = (e: MouseEvent) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return
    const href = node.getAttribute('href')
    if (!href || !href.startsWith('/') || href.startsWith('/api') || href.startsWith('/luci')) return
    e.preventDefault()
    navigate(href)
  }
  node.addEventListener('click', onClick)
  return { destroy: () => node.removeEventListener('click', onClick) }
}
