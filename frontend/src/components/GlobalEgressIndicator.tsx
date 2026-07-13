'use client';

import { Cloud, ShieldCheck } from 'lucide-react';
import { useConfig } from '@/contexts/ConfigContext';
import { summaryLocality, transcriptionLocality } from '@/lib/providerLocality';
import { cn } from '@/lib/utils';

/**
 * Persistent, always-visible signal of whether the CURRENTLY configured pipeline
 * keeps meeting data on-device. Reflects the durable summary + transcription
 * providers from ConfigContext. (Per-meeting diarization egress is surfaced at
 * the point the user picks it, inside RediarizeDialog.)
 */
export function GlobalEgressIndicator({ className }: { className?: string }) {
  const { modelConfig, transcriptModelConfig } = useConfig();

  const cloudPipelines: string[] = [];
  if (
    summaryLocality(modelConfig.provider, { customEndpoint: modelConfig.customOpenAIEndpoint }) ===
    'cloud'
  )
    cloudPipelines.push('summarization');
  if (transcriptionLocality(transcriptModelConfig.provider) === 'cloud')
    cloudPipelines.push('transcription');

  const anyCloud = cloudPipelines.length > 0;
  const Icon = anyCloud ? Cloud : ShieldCheck;
  const label = anyCloud ? 'Some data leaves this device' : 'Fully on device';
  const title = anyCloud
    ? `Cloud provider selected for: ${cloudPipelines.join(', ')}. Meeting data is sent to a third-party service.`
    : 'Summarization and transcription both run locally — your meeting data stays on this machine.';

  return (
    <div
      className={cn(
        'w-full flex items-center justify-center gap-1.5 px-3 py-1 text-xs font-medium',
        anyCloud ? 'text-amber-600 dark:text-amber-400' : 'text-emerald-600 dark:text-emerald-400',
        className
      )}
      title={title}
    >
      <Icon className="h-3.5 w-3.5" aria-hidden />
      <span>{label}</span>
    </div>
  );
}
