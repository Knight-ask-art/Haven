export const frontendGovernance = {
  tiers: {
    foundation: ["tailwindcss"],
    coreUI: ["shadcn", "radix-ui"],
    motion: ["motion"],
    approvedEffects: ["magic-ui"],
    reviewRequired: ["aceternity-ui"],
  },
  
  productionBoundaries: {
    restrictedPaths: [
      "@/components/external/**",
      "src/components/external/**"
    ],
    exceptionPaths: []
  },

  havenizationLifecycle: [
    "EXTERNAL",
    "QUARANTINED",
    "AUDITING",
    "HAVENIZING",
    "REVIEW",
    "APPROVED",
    "PROMOTED",
    "PRODUCTION"
  ],

  // 组件治理记录 (Component Manifest)
  externalComponents: {
    "animated-beam": {
      source: "magic-ui",
      tier: 2,
      lifecycle: "PROMOTED",
      contexts: {
        aiFlow: "allow",
        syncFlow: "allow",
        marketing: "review",
        reader: "deny",
        settings: "deny",
      }
    }
    // 更多组件在进入 Quarantine 流程后应注册于此
  }
};
