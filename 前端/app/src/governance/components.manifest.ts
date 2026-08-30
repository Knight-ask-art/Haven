export interface ComponentRecord {
  layer: 'ui' | 'shared' | 'feature' | 'external';
  status: 'production' | 'promoted' | 'rejected' | 'quarantined' | 'auditing' | 'havenizing' | 'review' | 'approved' | 'external';
  source: 'shadcn' | 'radix' | 'magic-ui' | 'aceternity' | 'custom' | 'native';
  owner?: string;
  havenized?: boolean;
  contexts?: string[];
}

export const componentManifest: Record<string, ComponentRecord> = {
  // Primitives
  Button: { layer: "ui", status: "production", source: "shadcn", owner: "design-system", havenized: true },
  Input: { layer: "ui", status: "production", source: "shadcn", owner: "design-system", havenized: true },
  Tabs: { layer: "ui", status: "production", source: "shadcn", owner: "design-system", havenized: true },
  
  // Custom Primitives
  HavenSidebar: { layer: "shared", status: "production", source: "custom", owner: "app-shell", havenized: true },
  LibraryCategoryBar: { layer: "feature", status: "production", source: "custom", owner: "library", havenized: true },
  MediaItem: { layer: "feature", status: "production", source: "custom", owner: "library", havenized: true },
};
