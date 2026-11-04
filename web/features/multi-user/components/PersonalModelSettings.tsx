"use client";

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { KeyRound, Plus, Trash2 } from "lucide-react";
import { apiFetch, apiUrl } from "@/lib/api";
import type { Catalog, CatalogProfile } from "@/lib/model-catalog-types";
import { invalidateLLMOptionsCache, listLLMOptions, type LLMOption } from "@/lib/llm-options";
import { useCapabilityAccess } from "@/components/access/CapabilityAccessContext";

export function PersonalModelSettings() {
  const { t } = useTranslation();
  const { refresh } = useCapabilityAccess();
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [assigned, setAssigned] = useState<LLMOption[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  useEffect(() => {
    let active = true;
    void Promise.all([
      apiFetch(apiUrl("/api/auth/profile/models"), { cache: "no-store" }).then(async (res) => {
        if (!res.ok) throw new Error(t("Failed to load personal models"));
        return (await res.json()).catalog as Catalog;
      }),
      listLLMOptions({ force: true }),
    ]).then(([own, options]) => {
      if (!active) return;
      setCatalog(own);
      setAssigned(options.options.filter((model) => model.source === "admin"));
    }).catch((e) => { if (active) setError(e instanceof Error ? e.message : t("Failed to load personal models")); });
    return () => { active = false; };
  }, [t]);

  function edit(update: (copy: Catalog) => void) {
    setNotice("");
    setCatalog((current) => {
      if (!current) return current;
      const copy = structuredClone(current);
      update(copy);
      return copy;
    });
  }
  function change(id: string, update: (profile: CatalogProfile) => void) {
    edit((copy) => { const profile = copy.services.llm.profiles.find((p) => p.id === id); if (profile) update(profile); });
  }
  function add() {
    edit((copy) => {
      const id = `p_${crypto.randomUUID().replaceAll("-", "")}`;
      const modelId = `m_${crypto.randomUUID().replaceAll("-", "")}`;
      copy.services.llm.profiles.push({ id, name: "", binding: "custom", base_url: "https://api.openai.com/v1", api_key: "", api_version: "", api_format: "openai_chat", models: [{ id: modelId, name: "", model: "" }] });
      if (!copy.services.llm.active_profile_id) {
        copy.services.llm.active_profile_id = id;
        copy.services.llm.active_model_id = modelId;
      }
    });
  }
  async function save(event: React.FormEvent) {
    event.preventDefault();
    if (!catalog || busy) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const response = await apiFetch(apiUrl("/api/auth/profile/models"), {
        method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ catalog }),
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.detail ?? t("Failed to save personal models"));
      setCatalog(result.catalog);
      invalidateLLMOptionsCache();
      await refresh();
      setNotice(t("Personal models saved"));
    } catch (e) { setError(e instanceof Error ? e.message : t("Failed to save personal models")); }
    finally { setBusy(false); }
  }
  const inputClass = "mt-1 w-full rounded-lg border border-[var(--border)] bg-[var(--background)] px-3 py-2 text-sm text-[var(--foreground)] disabled:opacity-50";

  return (
    <section id="models" className="mt-4 scroll-mt-6 rounded-2xl border border-[var(--border)] bg-[var(--card)] p-6 shadow-sm">
      <h2 className="flex items-center gap-2 text-sm font-semibold"><KeyRound size={16} />{t("Personal models and API keys")}</h2>
      <p className="mt-1 text-sm text-[var(--muted-foreground)]">{t("Use your own API key or a model assigned by your administrator. Personal keys belong only to your account.")}</p>
      {error && <p role="alert" className="mt-3 text-sm text-red-500">{error}</p>}
      {!catalog ? <p className="mt-4 text-sm text-[var(--muted-foreground)]">{t("Loading…")}</p> : (
        <form onSubmit={save} className="mt-4 space-y-4">
          {catalog.services.llm.profiles.map((profile) => (
            <fieldset key={profile.id} disabled={busy} className="rounded-xl border border-[var(--border)] p-4">
              <div className="mb-3 flex items-center justify-between gap-3">
                <label className="flex items-center gap-2 text-xs">
                  <input type="radio" name="personal-default" checked={catalog.services.llm.active_profile_id === profile.id} onChange={() => edit((copy) => { copy.services.llm.active_profile_id = profile.id; copy.services.llm.active_model_id = profile.models[0]?.id ?? null; })} />
                  {t("Use as default")}
                </label>
                <button type="button" aria-label={t("Remove personal model")} onClick={() => edit((copy) => {
                  copy.services.llm.profiles = copy.services.llm.profiles.filter((p) => p.id !== profile.id);
                  if (copy.services.llm.active_profile_id === profile.id) { const first = copy.services.llm.profiles[0]; copy.services.llm.active_profile_id = first?.id ?? null; copy.services.llm.active_model_id = first?.models[0]?.id ?? null; }
                })} className="rounded p-1 text-[var(--muted-foreground)] hover:text-red-500"><Trash2 size={15} /></button>
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <label className="text-xs">{t("Name")}<input required value={profile.name} onChange={(e) => change(profile.id, (p) => { p.name = e.target.value; })} className={inputClass} /></label>
                <label className="text-xs">{t("API format")}<select value={profile.api_format ?? "openai_chat"} onChange={(e) => change(profile.id, (p) => { p.api_format = e.target.value as CatalogProfile["api_format"]; p.binding = p.api_format === "anthropic" ? "anthropic" : "custom"; })} className={inputClass}>
                  <option value="openai_chat">OpenAI Chat Completions</option><option value="openai_responses">OpenAI Responses</option><option value="anthropic">Anthropic</option>
                </select></label>
                <label className="text-xs sm:col-span-2">{t("Base URL")}<input type="url" required value={profile.base_url} onChange={(e) => change(profile.id, (p) => { p.base_url = e.target.value; })} className={inputClass} /></label>
                <label className="text-xs">{t("Model identifier")}<input required value={profile.models[0]?.model ?? ""} onChange={(e) => change(profile.id, (p) => { const model = p.models[0]; if (model) { model.model = e.target.value; model.name = e.target.value; } })} placeholder="gpt-4.1" className={inputClass} /></label>
                <label className="text-xs">{t("Token / API key")}<input type="password" autoComplete="off" value={profile.api_key === "***" ? "" : profile.api_key} placeholder={profile.api_key === "***" ? t("Saved key; leave unchanged to keep it") : ""} onChange={(e) => change(profile.id, (p) => { p.api_key = e.target.value; })} className={inputClass} /></label>
              </div>
            </fieldset>
          ))}
          <div className="flex flex-wrap items-center gap-3">
            <button type="button" disabled={busy} onClick={add} className="inline-flex items-center gap-1.5 rounded-lg border border-[var(--border)] px-3 py-2 text-sm"><Plus size={14} />{t("Add personal model")}</button>
            <button disabled={busy} className="rounded-lg bg-[var(--foreground)] px-3 py-2 text-sm text-[var(--background)] disabled:opacity-50">{busy ? t("Saving...") : t("Save personal models")}</button>
            {notice && <span role="status" className="text-xs text-emerald-600">{notice}</span>}
          </div>
        </form>
      )}
      <div className="mt-5 border-t border-[var(--border)] pt-4">
        <h3 className="text-xs font-semibold">{t("Assigned models")}</h3>
        <p className="mt-1 text-xs text-[var(--muted-foreground)]">{t("Assigned connections are read-only. Their API keys stay with the administrator.")}</p>
        <ul className="mt-2 space-y-1 text-sm">{assigned.map((model) => <li key={`${model.profile_id}:${model.model_id}`}>{model.profile_name} · {model.model_name}</li>)}</ul>
      </div>
    </section>
  );
}
