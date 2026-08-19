/**
 * Accent colors for projects.
 *
 * A project stores a palette *slug*, never a hex value: each slug maps to a
 * `--project-*` theme token (globals.css) that has its own value in the light
 * and dark themes, so a project reads the same in both. Mirrors the speaker
 * palette in `speakerLabel.ts`.
 *
 * Must stay in sync with `PROJECT_COLORS` in
 * `frontend/src-tauri/src/database/repositories/project.rs`, which rejects any
 * slug outside this list.
 */
export const PROJECT_COLORS = [
  'violet',
  'blue',
  'cyan',
  'teal',
  'green',
  'amber',
  'orange',
  'rose',
] as const;

export type ProjectColor = (typeof PROJECT_COLORS)[number];

export interface ProjectColorClasses {
  /** Solid fill — the swatch, the dot, the card rail. */
  solid: string;
  /** Tinted pill: soft background, colored text and border. */
  chip: string;
  /** Colored glyph (folder icon, etc). */
  text: string;
}

// Literal class strings so Tailwind's scanner generates them — a template like
// `bg-project-${color}` would be purged out of the build.
const CLASSES: Record<ProjectColor, ProjectColorClasses> = {
  violet: {
    solid: 'bg-project-violet',
    chip: 'bg-project-violet/15 text-project-violet border-project-violet/30',
    text: 'text-project-violet',
  },
  blue: {
    solid: 'bg-project-blue',
    chip: 'bg-project-blue/15 text-project-blue border-project-blue/30',
    text: 'text-project-blue',
  },
  cyan: {
    solid: 'bg-project-cyan',
    chip: 'bg-project-cyan/15 text-project-cyan border-project-cyan/30',
    text: 'text-project-cyan',
  },
  teal: {
    solid: 'bg-project-teal',
    chip: 'bg-project-teal/15 text-project-teal border-project-teal/30',
    text: 'text-project-teal',
  },
  green: {
    solid: 'bg-project-green',
    chip: 'bg-project-green/15 text-project-green border-project-green/30',
    text: 'text-project-green',
  },
  amber: {
    solid: 'bg-project-amber',
    chip: 'bg-project-amber/15 text-project-amber border-project-amber/30',
    text: 'text-project-amber',
  },
  orange: {
    solid: 'bg-project-orange',
    chip: 'bg-project-orange/15 text-project-orange border-project-orange/30',
    text: 'text-project-orange',
  },
  rose: {
    solid: 'bg-project-rose',
    chip: 'bg-project-rose/15 text-project-rose border-project-rose/30',
    text: 'text-project-rose',
  },
};

/** Human label for the swatch tooltips. */
export const PROJECT_COLOR_LABELS: Record<ProjectColor, string> = {
  violet: 'Violet',
  blue: 'Blue',
  cyan: 'Cyan',
  teal: 'Teal',
  green: 'Green',
  amber: 'Amber',
  orange: 'Orange',
  rose: 'Rose',
};

function isProjectColor(value: string): value is ProjectColor {
  return (PROJECT_COLORS as readonly string[]).includes(value);
}

/** Stable hash so a color-less project always lands on the same palette slot. */
function hash(value: string): number {
  let h = 0;
  for (let i = 0; i < value.length; i++) {
    h = (h * 31 + value.charCodeAt(i)) | 0;
  }
  return Math.abs(h);
}

/**
 * The color to render a project with. Projects created before the picker
 * existed have no stored color, so one is derived from the id — stable across
 * renders and sessions, and never leaves a project uncolored.
 */
export function resolveProjectColor(project: {
  id: string;
  color?: string | null;
}): ProjectColor {
  const stored = project.color?.trim().toLowerCase();
  if (stored && isProjectColor(stored)) return stored;
  return PROJECT_COLORS[hash(project.id) % PROJECT_COLORS.length];
}

export function projectColorClasses(color: ProjectColor): ProjectColorClasses {
  return CLASSES[color];
}

/** Shorthand for the common "resolve, then take the classes" pair. */
export function projectClasses(project: {
  id: string;
  color?: string | null;
}): ProjectColorClasses {
  return CLASSES[resolveProjectColor(project)];
}
