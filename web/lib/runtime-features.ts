type Module = { id: string; features?: Record<string, boolean> };

/** Older inventories have no native feature gates; preserve their behavior. */
export function runtimeFeatureAvailable(
  modules: Module[] | null,
  feature: string,
): boolean {
  if (!modules) return false;
  const core = modules.find((module) => module.id === "core");
  return core?.features?.native_runtime
    ? core.features[feature] === true
    : true;
}
