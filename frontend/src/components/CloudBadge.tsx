'use client';

import { Cloud, ShieldCheck } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { Locality } from '@/lib/providerLocality';

interface CloudBadgeProps {
  locality: Locality;
  className?: string;
  /** When false, render only the icon (for tight/collapsed layouts). */
  showLabel?: boolean;
}

/**
 * Small pill that tells the user whether the selected provider keeps data on
 * this device ("On device", green) or sends it to a third-party cloud
 * ("Leaves this device", amber). Reinforces the app's privacy-first promise at
 * each point the user picks a provider.
 */
export function CloudBadge({ locality, className, showLabel = true }: CloudBadgeProps) {
  const isCloud = locality === 'cloud';
  const Icon = isCloud ? Cloud : ShieldCheck;
  const label = isCloud ? 'Leaves this device' : 'On device';
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium whitespace-nowrap',
        isCloud
          ? 'bg-amber-500/15 text-amber-600 dark:text-amber-400'
          : 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400',
        className
      )}
      title={
        isCloud
          ? 'This provider sends your meeting data to a third-party cloud service.'
          : 'This provider runs entirely on your machine — data stays local.'
      }
    >
      <Icon className="h-3 w-3" aria-hidden />
      {showLabel && <span>{label}</span>}
    </span>
  );
}
