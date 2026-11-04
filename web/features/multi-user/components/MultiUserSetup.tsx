"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useTranslation } from "react-i18next";
import { ArrowLeft, ShieldCheck } from "lucide-react";
import { checkIsFirstUser, invalidateAuthStatusCache } from "@/lib/auth";
import { apiFetch, apiUrl } from "@/lib/api";

export function MultiUserSetup() {
  const { t } = useTranslation();
  const [first, setFirst] = useState<boolean | null>(null);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => { void checkIsFirstUser().then(setFirst); }, []);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true); setError("");
    try {
      const response = await apiFetch(apiUrl("/api/auth/setup"), {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ username, password }),
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.detail ?? t("Failed to enable multi-user mode"));
      invalidateAuthStatusCache();
      window.location.assign("/admin/users");
    } catch (error) {
      setError(error instanceof Error ? error.message : t("Failed to enable multi-user mode"));
      setBusy(false);
    }
  }

  return (
    <div className="min-h-screen overflow-y-auto bg-[var(--background)] px-4 py-10">
      <div className="mx-auto max-w-md">
        <Link href="/" className="mb-6 inline-flex items-center gap-1.5 text-sm text-[var(--muted-foreground)]"><ArrowLeft size={16} />{t("Back")}</Link>
        <div className="rounded-2xl border border-[var(--border)] bg-[var(--card)] p-6 shadow-sm">
          <ShieldCheck size={28} className="mb-4 text-[var(--primary)]" />
          <h1 className="font-serif text-xl font-semibold">{t("Enable multi-user mode")}</h1>
          <p className="mt-2 text-sm text-[var(--muted-foreground)]">{first ? t("Create an administrator account to manage users. Your existing workspace stays with the administrator.") : t("Sign in with an existing administrator account to enable multi-user mode.")}</p>
          <form onSubmit={submit} className="mt-6 space-y-4">
            <label className="block text-sm">{t("Username")}
              <input required autoComplete="username" value={username} onChange={(e) => setUsername(e.target.value)} disabled={busy || first === null} className="mt-1 w-full rounded-lg border border-[var(--border)] bg-[var(--background)] px-3 py-2" />
            </label>
            <label className="block text-sm">{t("Password")}
              <input type="password" required minLength={first ? 8 : undefined} autoComplete={first ? "new-password" : "current-password"} value={password} onChange={(e) => setPassword(e.target.value)} disabled={busy || first === null} className="mt-1 w-full rounded-lg border border-[var(--border)] bg-[var(--background)] px-3 py-2" />
            </label>
            {error && <p role="alert" className="text-sm text-red-500">{error}</p>}
            <button disabled={busy || first === null} className="w-full rounded-lg bg-[var(--foreground)] px-4 py-2 text-sm text-[var(--background)] disabled:opacity-50">{busy ? t("Saving...") : t("Enable multi-user mode")}</button>
          </form>
        </div>
      </div>
    </div>
  );
}
