'use client'

import { useEffect, useState } from 'react'
import { usePlatform } from '@/hooks/usePlatform'
import { WindowControls } from '@/components/WindowChrome/WindowControls'
import { Logomark } from '@/components/brand/Logomark'

export const TITLEBAR_HEIGHT = '2.5rem'

/**
 * Custom window titlebar, rendered only inside Tauri. Windows/Linux run
 * undecorated and get custom window controls; macOS keeps its native traffic
 * lights (titleBarStyle Overlay) so we only reserve space for them.
 *
 * Publishes its height as --titlebar-height on <html> so the shell (sidebar,
 * page roots) can offset with h-[calc(100vh-var(--titlebar-height))] and
 * behave identically in browser preview, where the var stays 0px.
 */
export function Titlebar() {
  const [inTauri, setInTauri] = useState(false)
  const platform = usePlatform()

  useEffect(() => {
    setInTauri(typeof window.__TAURI_INTERNALS__ !== 'undefined')
  }, [])

  useEffect(() => {
    if (!inTauri) return
    document.documentElement.style.setProperty('--titlebar-height', TITLEBAR_HEIGHT)
    return () => {
      document.documentElement.style.setProperty('--titlebar-height', '0px')
    }
  }, [inTauri])

  if (!inTauri) return null

  return (
    <header
      data-tauri-drag-region
      className="fixed top-0 inset-x-0 z-[60] h-10 flex items-center justify-between bg-background border-b border-border select-none"
    >
      {/* pointer-events-none so clicks fall through to the drag region */}
      <div
        data-tauri-drag-region
        className={`flex items-center gap-2 ${platform === 'macos' ? 'pl-20' : 'pl-4'}`}
      >
        <Logomark size={16} className="pointer-events-none" />
        <span className="text-caption font-medium text-muted-foreground pointer-events-none">
          Meetily
        </span>
      </div>
      {platform !== 'macos' && <WindowControls />}
    </header>
  )
}
