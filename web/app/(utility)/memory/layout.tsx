'use client'

import { usePathname } from 'next/navigation'
import { useModules } from '@/hooks/useModules'
import NativeMemoryWorkspace, { nativeMemoryTarget } from '@/components/memory/NativeMemoryWorkspace'

export default function MemoryLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  const pathname = usePathname() ?? '/memory'
  const { modules } = useModules()
  const target = nativeMemoryTarget(pathname)
  const native = modules?.some(module => module.id === 'memory' && module.features?.native_documents)
  return (
    <main className="flex h-full min-h-0 flex-col bg-[var(--background)]">
      {native && target ? <NativeMemoryWorkspace key={target.join('/')} layer={target[0]} docKey={target[1]} /> : children}
    </main>
  );
}
