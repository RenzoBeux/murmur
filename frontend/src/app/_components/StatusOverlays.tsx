interface StatusOverlaysProps {
  // Status flags
  isProcessing: boolean;      // Processing transcription after recording stops
  isSaving: boolean;          // Saving transcript to database

  // Layout
  sidebarCollapsed: boolean;  // For responsive margin calculation
  askAiOpen?: boolean;        // Inset by the Ask-AI drawer width when it is open
}

// Internal reusable component for individual status overlays
interface StatusOverlayProps {
  show: boolean;
  message: string;
  sidebarCollapsed: boolean;
  askAiOpen?: boolean;
}

function StatusOverlay({ show, message, sidebarCollapsed, askAiOpen }: StatusOverlayProps) {
  if (!show) return null;

  return (
    <div className={`fixed bottom-4 left-0 z-10 ${askAiOpen ? 'right-[380px] xl:right-[420px]' : 'right-0'}`}>
      <div
        className={`flex justify-center pl-3 md:pl-8 transition-[margin] duration-300 ${
          sidebarCollapsed ? 'ml-16' : 'ml-16 md:ml-64'
        }`}
      >
        <div className="w-full px-4 md:px-0 md:w-2/3 max-w-[750px] flex justify-center">
          <div className="bg-elevated/90 backdrop-blur-md border border-border rounded-full shadow-glass px-4 py-2 flex items-center space-x-2">
            <div className="animate-spin rounded-full h-4 w-4 border-b-2 border-brand"></div>
            <span className="text-sm text-foreground">{message}</span>
          </div>
        </div>
      </div>
    </div>
  );
}

// Main exported component - renders multiple status overlays
export function StatusOverlays({
  isProcessing,
  isSaving,
  sidebarCollapsed,
  askAiOpen
}: StatusOverlaysProps) {
  return (
    <>
      {/* Processing status overlay - shown after recording stops while finalizing transcription */}
      <StatusOverlay
        show={isProcessing}
        message="Finalizing transcription..."
        sidebarCollapsed={sidebarCollapsed}
        askAiOpen={askAiOpen}
      />

      {/* Saving status overlay - shown while saving transcript to database */}
      <StatusOverlay
        show={isSaving}
        message="Saving transcript..."
        sidebarCollapsed={sidebarCollapsed}
        askAiOpen={askAiOpen}
      />
    </>
  );
}
