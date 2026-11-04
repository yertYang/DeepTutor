import { Suspense } from 'react'
import NativeStudyWorkspace from '@/components/learning/NativeStudyWorkspace'
export default function Page() {
  return (
    <Suspense>
      <NativeStudyWorkspace initialKind="mastery_path" />
    </Suspense>
  )
}
