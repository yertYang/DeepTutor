"use client";

import { WorkspaceError } from "@/components/common/WorkspaceFeedback";

export default function RouteError({ reset }: { reset: () => void }) {
  return <WorkspaceError reset={reset} />;
}
