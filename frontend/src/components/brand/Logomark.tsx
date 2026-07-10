'use client'

import React, { useId } from 'react'

interface LogomarkProps {
  size?: number
  className?: string
}

/**
 * The Meetily logomark: a charcoal tile with four violet waveform bars.
 * Inline SVG so it stays crisp at any size and needs no asset loading.
 * Same art as src-tauri/app-icon.svg (the app-icon source of truth).
 */
export function Logomark({ size = 24, className }: LogomarkProps) {
  const gradientId = useId()
  return (
    <svg
      width={size}
      height={size}
      viewBox="102 102 820 820"
      xmlns="http://www.w3.org/2000/svg"
      className={className}
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#9D8CFF" />
          <stop offset="1" stopColor="#7C6CFF" />
        </linearGradient>
      </defs>
      <rect x="102" y="102" width="820" height="820" rx="184" fill="#1A1A1E" />
      <rect
        x="106"
        y="106"
        width="812"
        height="812"
        rx="180"
        fill="none"
        stroke="#FFFFFF"
        strokeOpacity="0.08"
        strokeWidth="8"
      />
      <g fill={`url(#${gradientId})`}>
        <rect x="245" y="272" width="90" height="480" rx="45" />
        <rect x="393" y="372" width="90" height="280" rx="45" />
        <rect x="541" y="372" width="90" height="280" rx="45" />
        <rect x="689" y="272" width="90" height="480" rx="45" />
      </g>
    </svg>
  )
}
