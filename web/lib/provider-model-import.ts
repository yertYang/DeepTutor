import type { Catalog, Discovery, ProviderRef } from "./model-catalog-types";
import { modelProviderRef, providerIdentity, providerRegistry } from "./provider-registry";
import { randomUuid } from "./random-uuid";

/** Only an authenticated listing may add models. Keep manual models and defaults. */
export function importDiscoveredChatModels(
  catalog: Catalog, sourceId: string, discovery: Discovery, ref: ProviderRef,
): number {
  if (discovery.status !== "connected") return 0;
  const source = providerRegistry(catalog).find(item => item.id === sourceId);
  if (!source) return 0;
  const models = [...new Set(discovery.models.filter(model => {
    if (model.services?.length) return model.services.includes("llm");
    // /models sometimes has no modality metadata. Exclude unambiguous standard
    // non-chat model IDs; preserve unknown/custom text model names.
    return !/^(text-embedding|embedding|whisper|tts-|dall-e|gpt-image|sora|omni-moderation|text-moderation)/i.test(model.id);
  }).map(model => model.id.trim()).filter(Boolean))];
  if (!models.length) return 0;
  const bucket = catalog.services.llm;
  const existing = new Set(bucket.profiles.flatMap(profile => profile.models
    .filter(model => providerIdentity(modelProviderRef("llm", profile, model)) === sourceId)
    .map(model => model.model)));
  const missing = models.filter(model => !existing.has(model));
  if (!missing.length) return 0;
  let profile = bucket.profiles.find(item =>
    !item.provider_only && providerIdentity(modelProviderRef("llm", item)) === sourceId);
  if (!profile) {
    profile = {
      id: `llm-profile-${randomUuid()}`, name: source.name,
      binding: ref.binding || source.provider, provider_ref: ref,
      api_key: "", base_url: "", api_version: "", models: [],
    };
    bucket.profiles.push(profile);
  }
  for (const model of missing) profile.models.push({
    id: `llm-model-${randomUuid()}`, name: model, model, provider_ref: ref,
  });
  const active = bucket.profiles.find(item => item.id === bucket.active_profile_id);
  if (!active?.models.some(model => model.id === bucket.active_model_id)) {
    bucket.active_profile_id = profile.id;
    bucket.active_model_id = profile.models[0].id;
  }
  return missing.length;
}
