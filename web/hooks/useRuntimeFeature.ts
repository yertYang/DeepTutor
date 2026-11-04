"use client";

import { useModules } from "@/hooks/useModules";
import { runtimeFeatureAvailable } from "@/lib/runtime-features";

export function useRuntimeFeature(feature: string): boolean {
  const { modules } = useModules();
  return runtimeFeatureAvailable(modules, feature);
}
