-- Accent color for a project, so it is recognizable at a glance in listings,
-- chips and headers. Stored as a palette slug (violet/blue/cyan/teal/green/
-- amber/orange/rose), never a raw hex value: the slug maps to a `--project-*`
-- theme token that has a distinct value per light/dark theme, the same way the
-- speaker palette works.
--
-- NULL means "not chosen" — projects created before this migration. The UI
-- derives a stable color from the project id in that case, so every project
-- always renders with one.
ALTER TABLE projects ADD COLUMN color TEXT;
