"use client";

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ConfirmDialog } from "@/components/ui/ConfirmDialog";
import {
  getGuardianMaterials, getGuardianReport, getGuardianRestrictions,
  listGuardianRelationships, resetLearnerCredentials, revokeMyGuardianRelationship,
  saveGuardianMaterials, saveGuardianRestrictions,
  type GuardianMaterial, type GuardianRelationship, type GuardianReport, type GuardianRestrictions,
} from "@/lib/guardian-api";

const fieldClass = "w-full rounded-md border border-[var(--border)] bg-[var(--background)] px-3 py-2 text-sm";
const buttonClass = "rounded-md border border-[var(--border)] px-3 py-2 text-sm disabled:opacity-40";

export function GuardianAccessPanel({ userId }: { userId: string }) {
  const { t } = useTranslation();
  const [relationships, setRelationships] = useState<GuardianRelationship[] | null>(null);
  const [selectedId, setSelectedId] = useState("");
  const [error, setError] = useState("");
  const [revoke, setRevoke] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let active = true;
    listGuardianRelationships().then((rows) => {
      if (active) {
        setRelationships(rows);
        setSelectedId(rows[0]?.id ?? "");
      }
    }).catch((reason: unknown) => { if (active) setError((reason as Error).message); });
    return () => { active = false; };
  }, [userId]);
  const selected = relationships?.find((row) => row.id === selectedId);
  return (
    <section className="space-y-4 rounded-xl border border-[var(--border)] p-5" aria-label={t("Guardian relationships")}>
      <h2 className="font-medium">{t("Guardian relationships")}</h2>
      {error && <p role="alert" className="text-sm text-red-600">{t(error)}</p>}
      {!relationships ? <p role="status">{t("Loading guardian relationships…")}</p> : relationships.length === 0 ? (
        <p className="text-sm text-[var(--muted-foreground)]">{t("No active guardian relationships.")}</p>
      ) : (
        <>
          <select className={fieldClass} value={selectedId} onChange={(event) => setSelectedId(event.target.value)} aria-label={t("Guardian relationships")}>
            {relationships.map((row) => <option key={row.id} value={row.id}>{row.guardian_user_id === userId ? row.learner_username : row.guardian_username}</option>)}
          </select>
          {selected?.guardian_user_id === userId && <LearnerAccess key={selected.id} relationship={selected} />}
          <button type="button" className={buttonClass} disabled={busy} onClick={() => setRevoke(true)}>{t("Revoke guardian access")}</button>
        </>
      )}
      <ConfirmDialog open={revoke} title={t("Revoke guardian access")} busy={busy} tone="danger" confirmLabel={t("Revoke access")} onCancel={() => setRevoke(false)} onConfirm={() => {
        if (!selected || busy) return;
        setBusy(true);
        void revokeMyGuardianRelationship(selected.id).then(() => {
          const next = relationships?.filter((row) => row.id !== selected.id) ?? [];
          setRelationships(next); setSelectedId(next[0]?.id ?? ""); setRevoke(false);
        }).catch((reason: unknown) => setError((reason as Error).message)).finally(() => setBusy(false));
      }} />
    </section>
  );
}

function LearnerAccess({ relationship }: { relationship: GuardianRelationship }) {
  const { t } = useTranslation();
  const id = relationship.learner_user_id;
  const [materials, setMaterials] = useState<GuardianMaterial[] | null>(null);
  const [restrictions, setRestrictions] = useState<GuardianRestrictions | null>(null);
  const [report, setReport] = useState<GuardianReport | null>(null);
  const [password, setPassword] = useState("");
  const [confirmReset, setConfirmReset] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const can = (permission: string) => relationship.permissions.includes(permission);
  useEffect(() => {
    let active = true;
    const work: Promise<void>[] = [];
    if (relationship.permissions.includes("assign_materials")) work.push(getGuardianMaterials(id).then((rows) => { if (active) setMaterials(rows); }));
    if (relationship.permissions.includes("manage_restrictions")) work.push(getGuardianRestrictions(id).then((value) => { if (active) setRestrictions(value.restrictions); }));
    if (relationship.permissions.includes("view_reports")) work.push(getGuardianReport(id).then((value) => { if (active) setReport(value); }));
    void Promise.all(work).catch((reason: unknown) => { if (active) setError((reason as Error).message); }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [id, relationship.permissions]);
  const run = async (operation: () => Promise<unknown>) => {
    setBusy(true); setError(""); setMessage("");
    try { await operation(); setMessage(t("Saved")); }
    catch (reason) { setError(reason instanceof Error ? reason.message : t("Request failed")); }
    finally { setBusy(false); }
  };
  const saveMaterials = async () => {
    const selected = materials?.filter((item) => item.assigned) ?? [];
    await saveGuardianMaterials(id, selected.map((item) => item.book_id));
    setReport((current) => current ? {
      ...current,
      assigned_materials: selected.map((item) => ({ book_id: item.book_id, title: item.title, permission: item.permission })),
    } : null);
  };
  if (loading) return <p role="status">{t("Loading…")}</p>;
  return (
    <div className="space-y-5">
      {error && <p role="alert" className="text-sm text-red-600">{t(error)}</p>}
      {message && <p role="status" className="text-sm text-emerald-600">{message}</p>}
      {report && <p className="text-sm">{t("{{materials}} approved materials · {{resources}} enabled resources", { materials: report.assigned_materials.length, resources: report.grant_summary.model_count + report.grant_summary.knowledge_base_count + report.grant_summary.skill_count })}</p>}
      {materials && <fieldset disabled={busy} className="space-y-2">
        <legend className="mb-2 font-medium">{t("Manage materials")}</legend>
        {materials.map((item) => <label key={item.book_id} className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={item.assigned} onChange={(event) => setMaterials((rows) => rows?.map((row) => row.book_id === item.book_id ? { ...row, assigned: event.target.checked } : row) ?? null)} />
          {item.title || item.book_id}
        </label>)}
        <button type="button" className={buttonClass} onClick={() => void run(saveMaterials)}>{t("Save")}</button>
      </fieldset>}
      {restrictions && <fieldset disabled={busy} className="space-y-3">
        <legend className="mb-2 font-medium">{t("Manage restrictions")}</legend>
        <label className="block space-y-1 text-sm"><span>{t("Age band")}</span><select className={fieldClass} value={restrictions.age_band} onChange={(event) => setRestrictions({ ...restrictions, age_band: event.target.value as GuardianRestrictions["age_band"] })}>
          {["6-8", "9-12", "13-15"].map((age) => <option key={age} value={age}>{age}</option>)}
        </select></label>
        {(["chat", "reading"] as const).map((surface) => <label key={surface} className="flex items-center gap-2 text-sm"><input type="checkbox" checked={restrictions.allowed_surfaces.includes(surface)} onChange={(event) => setRestrictions({ ...restrictions, allowed_surfaces: event.target.checked ? [...restrictions.allowed_surfaces, surface] : restrictions.allowed_surfaces.filter((value) => value !== surface) })} />{t(surface === "chat" ? "Chat" : "Reading")}</label>)}
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={restrictions.allow_upload} onChange={(event) => setRestrictions({ ...restrictions, allow_upload: event.target.checked })} />{t("Allow uploads")}</label>
        <button type="button" className={buttonClass} disabled={restrictions.allowed_surfaces.length === 0} onClick={() => void run(() => saveGuardianRestrictions(id, restrictions))}>{t("Save")}</button>
      </fieldset>}
      {can("reset_credentials") && <fieldset disabled={busy} className="space-y-2">
        <label className="block space-y-1 text-sm"><span>{t("New learner password")}</span><input className={fieldClass} type="password" value={password} autoComplete="new-password" onChange={(event) => setPassword(event.target.value)} /></label>
        <button type="button" className={buttonClass} disabled={password.length < 8} onClick={() => setConfirmReset(true)}>{t("Reset learner credentials")}</button>
      </fieldset>}
      <ConfirmDialog open={confirmReset} title={t("Reset learner credentials")} tone="danger" confirmLabel={t("Reset learner credentials")} onCancel={() => setConfirmReset(false)} onConfirm={() => {
        setConfirmReset(false);
        void run(async () => { await resetLearnerCredentials(id, password); setPassword(""); });
      }} />
    </div>
  );
}
