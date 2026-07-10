'use client'

import React from 'react'
import { Logomark } from '@/components/brand/Logomark'
import { cn } from '@/lib/utils'

interface WordmarkProps {
  markSize?: number
  className?: string
}

/** Logomark + "Meetily" lockup for the sidebar header and other chrome. */
export function Wordmark({ markSize = 24, className }: WordmarkProps) {
  return (
    <span className={cn('inline-flex items-center gap-2', className)}>
      <Logomark size={markSize} />
      <span className="text-[17px] font-semibold tracking-tight text-foreground">
        Meetily
      </span>
    </span>
  )
}
