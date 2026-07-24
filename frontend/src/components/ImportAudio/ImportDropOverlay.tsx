import React from 'react';
import { Upload } from 'lucide-react';
import { getAudioFormatsDisplayList } from '@/constants/audioFormats';

interface ImportDropOverlayProps {
  visible: boolean;
  /** Defaults keep the original audio-import copy. */
  title?: string;
  subtitle?: string;
}

export function ImportDropOverlay({ visible, title, subtitle }: ImportDropOverlayProps) {
  if (!visible) return null;

  return (
    <div
      className="fixed inset-0 z-50 bg-background/70 backdrop-blur-sm
                 flex items-center justify-center pointer-events-none
                 transition-opacity duration-200"
    >
      <div className="border-2 border-dashed border-brand rounded-2xl
                      p-12 text-center bg-brand/10 shadow-glass
                      transform scale-100 transition-transform">
        <Upload className="h-16 w-16 text-brand mx-auto mb-4" />
        <p className="text-xl font-medium text-foreground">
          {title ?? 'Drop audio file to import'}
        </p>
        <p className="text-sm text-muted-foreground mt-2">
          {subtitle ?? getAudioFormatsDisplayList()}
        </p>
      </div>
    </div>
  );
}
