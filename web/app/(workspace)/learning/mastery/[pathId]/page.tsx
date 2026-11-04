import { Suspense } from 'react'
import NativeStudyWorkspace from '@/components/learning/NativeStudyWorkspace'
export default async function Page({ params }: { params: Promise<{ pathId: string }> }) {
  const { pathId } = await params
  return (
    <Suspense>
      <NativeStudyWorkspace initialKind="mastery_path" initialArtifactId={pathId} />
    </Suspense>
  )
}
