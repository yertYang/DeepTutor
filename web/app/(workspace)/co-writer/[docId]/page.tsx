"use client";

import dynamic from "next/dynamic";
import { useParams } from "next/navigation";

import NoteWorkspace from "@/features/notes/components/NoteWorkspace";
import { useModules } from "@/hooks/useModules";

const Companion = dynamic(() => import("@/features/notes/components/NoteCompanion"), { ssr: false });
const NativeWorkspace = dynamic(() => import("@/features/notes/components/NativeNoteWorkspace"), { ssr: false });

export default function CoWriterPage() {
  const params = useParams<{ docId?: string | string[] }>();
  const { modules } = useModules();
  const rawDocId = params?.docId;
  const docId = Array.isArray(rawDocId)
    ? (rawDocId[0] ?? "")
    : (rawDocId ?? "");

  if (!modules) return null;
  return modules.some(module => module.id === 'co-writer' && module.features?.native_editor)
    ? <NativeWorkspace key={docId} docId={docId} />
    : <NoteWorkspace key={docId} docId={docId} companion={Companion} />;
}
