export function shouldAttachInboxDecision(pathname: string, tab: string | undefined): boolean {
  return pathname === '/' && (tab === undefined || tab === 'inbox')
}
