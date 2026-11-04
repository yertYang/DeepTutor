"use client";

import { WorkspaceError } from "@/components/common/WorkspaceFeedback";

// This boundary renders inside WorkspaceLayout, retaining navigation and chrome.
export default function WorkspaceRouteError({ reset }: { reset: () => void }) {
  return <WorkspaceError reset={reset} />;
}
