import NativeReadingReader from '@/components/reading/native/NativeReadingReader'
export default async function ReadingRoute({
  params,
}: {
  params: Promise<{ workspaceId: string }>
}) {
  const { workspaceId } = await params
  return <NativeReadingReader key={workspaceId} itemId={workspaceId} />
}
