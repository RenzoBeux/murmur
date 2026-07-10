import React from "react";
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "./ui/dialog";
import { VisuallyHidden } from "./ui/visually-hidden";
import { About } from "./About";
import { Logomark } from "./brand/Logomark";
import { Wordmark } from "./brand/Wordmark";

interface LogoProps {
    isCollapsed: boolean;
}

const Logo = React.forwardRef<HTMLButtonElement, LogoProps>(({ isCollapsed }, ref) => {
  return (
    <Dialog aria-describedby={undefined}>
      <DialogTrigger asChild>
        <button
          ref={ref}
          className="flex items-center justify-start mb-2 cursor-pointer bg-transparent border-none p-0 hover:opacity-80 transition-opacity"
          aria-label="About Meetily"
        >
          {isCollapsed ? <Logomark size={32} /> : <Wordmark markSize={28} />}
        </button>
      </DialogTrigger>
      <DialogContent>
        <VisuallyHidden>
          <DialogTitle>About Meetily</DialogTitle>
        </VisuallyHidden>
        <About />
      </DialogContent>
    </Dialog>
  );
});

Logo.displayName = "Logo";

export default Logo;
