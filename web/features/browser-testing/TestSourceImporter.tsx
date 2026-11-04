'use client'

import {forwardRef, useImperativeHandle, useRef, useState} from 'react'
import {useTranslation} from 'react-i18next'
import {FileSpreadsheet, Loader2, Upload} from 'lucide-react'
import {apiFetch} from '@/lib/api'
import {scopedUrl} from '@/lib/workspace-scope'

export type TestSource = {id:string;name:string;revision:number;line_count:number;workbook:boolean;size:number}
export type TestSourceImportHandle = {pick:()=>void;upload:(files:File[])=>void}
export const TEST_SOURCE_ACCEPT = '.xlsx,.docx,.pdf,.pptx,.txt,.md,.markdown,.csv,.json'

export default forwardRef<TestSourceImportHandle, {workspaceId:string;disabled?:boolean;onImported?:(sources:TestSource[])=>void;onBusy?:(busy:boolean)=>void}>(function TestSourceImporter({workspaceId,disabled,onImported,onBusy},ref) {
  const {t}=useTranslation()
  const input=useRef<HTMLInputElement>(null)
  const [busy,setBusy]=useState(false)
  const [error,setError]=useState('')
  const importing=useRef(false)
  const currentScope=useRef(workspaceId)
  currentScope.current=workspaceId
  const upload=async(files:File[])=>{
    if (!files.length || disabled || importing.current) return
    if (files.length>32 || files.some(file=>file.size>16*1024*1024) || files.reduce((sum,file)=>sum+file.size,0)>64*1024*1024) {
      setError(t('Import up to 32 files, 16 MiB each and 64 MiB in total.')); return
    }
    importing.current=true;setBusy(true);setError('');onBusy?.(true)
    try {
      const form=new FormData(); files.forEach(file=>form.append('files',file,file.name))
      const response=await apiFetch(scopedUrl('/api/browser-testing/sources',workspaceId),{method:'POST',body:form})
      const value=await response.json()
      if (!response.ok) throw new Error(value.detail || t('Import failed'))
      if (currentScope.current===workspaceId) onImported?.(value.sources as TestSource[])
    } catch(e) {setError(e instanceof Error?e.message:String(e))}
    finally {importing.current=false;setBusy(false);onBusy?.(false)}
  }
  useImperativeHandle(ref,()=>({pick:()=>input.current?.click(),upload:files=>{void upload(files)}}))
  return <div className="space-y-2 text-sm">
    <input ref={input} type="file" multiple accept={TEST_SOURCE_ACCEPT} className="hidden" aria-label={t('Import test files')} disabled={busy||disabled} onChange={event=>{const files=Array.from(event.target.files??[]);event.target.value='';void upload(files)}}/>
    <button type="button" disabled={busy||disabled} onClick={()=>input.current?.click()} className="inline-flex items-center gap-2 rounded-lg border border-[var(--border)] px-3 py-2 text-sm hover:bg-[var(--muted)] disabled:opacity-50">
      {busy?<Loader2 size={15} className="animate-spin"/>:<Upload size={15}/>} {t(busy?'Reading test files…':'Import test files')}
    </button>
    <p className="flex items-center gap-1 text-xs text-[var(--muted-foreground)]"><FileSpreadsheet size={13}/>{t('Import requirements and Excel cases. Test results are saved to a new workbook; originals are preserved.')}</p>
    {error&&<p role="alert" className="text-xs text-red-700 dark:text-red-300">{error}</p>}
  </div>
})
