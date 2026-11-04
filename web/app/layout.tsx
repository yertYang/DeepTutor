import { BRAND_ASSETS, PRODUCT_NAME } from '@/lib/product'
import type { Metadata, Viewport } from 'next'
import localFont from 'next/font/local'
import './globals.css'
import '@/features/ui-layout/classic/classic.css'
import '@/features/ui-layout/modern/modern.css'
import ThemeScript from '@/components/ThemeScript'
import ToastViewport from '@/components/common/ToastViewport'
import MotionProvider from '@/components/common/MotionProvider'
import SettingsReturnTracker from '@/components/settings/SettingsReturnTracker'
import { WorkspaceNavigation } from '@/components/workspaces/WorkspaceNavigation'
import { AppShellProvider } from '@/context/AppShellContext'
import { I18nClientBridge } from '@/i18n/I18nClientBridge'

// Keep build and packaged runtime fonts local so builds work without Google Fonts.
const fontSans = localFont({
  src: [
    {
      path: './fonts/geist-100.ttf',
      weight: '100',
      style: 'normal',
    },
    {
      path: './fonts/geist-200.ttf',
      weight: '200',
      style: 'normal',
    },
    {
      path: './fonts/geist-300.ttf',
      weight: '300',
      style: 'normal',
    },
    {
      path: './fonts/geist-400.ttf',
      weight: '400',
      style: 'normal',
    },
    {
      path: './fonts/geist-500.ttf',
      weight: '500',
      style: 'normal',
    },
    {
      path: './fonts/geist-600.ttf',
      weight: '600',
      style: 'normal',
    },
    {
      path: './fonts/geist-700.ttf',
      weight: '700',
      style: 'normal',
    },
    {
      path: './fonts/geist-800.ttf',
      weight: '800',
      style: 'normal',
    },
    {
      path: './fonts/geist-900.ttf',
      weight: '900',
      style: 'normal',
    },
  ],
  display: 'swap',
  variable: '--font-sans',
})

const fontSerif = localFont({
  src: [
    {
      path: './fonts/lora-400.ttf',
      weight: '400',
      style: 'normal',
    },
    {
      path: './fonts/lora-500.ttf',
      weight: '500',
      style: 'normal',
    },
    {
      path: './fonts/lora-600.ttf',
      weight: '600',
      style: 'normal',
    },
    {
      path: './fonts/lora-700.ttf',
      weight: '700',
      style: 'normal',
    },
  ],
  display: 'swap',
  variable: '--font-serif',
})

export const viewport: Viewport = { interactiveWidget: 'resizes-content' }

export const metadata: Metadata = {
  title: PRODUCT_NAME,
  description: 'Lorevia：支持持续学习与知识积累的智能学习工作区',
  icons: {
    icon: [
      { url: BRAND_ASSETS.logo, type: 'image/svg+xml' },
      { url: BRAND_ASSETS.favicon16, sizes: '16x16', type: 'image/png' },
      { url: BRAND_ASSETS.favicon32, sizes: '32x32', type: 'image/png' },
    ],
    apple: BRAND_ASSETS.appleTouchIcon,
  },
}

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html
      lang="en"
      suppressHydrationWarning
      data-scroll-behavior="smooth"
      className={`${fontSans.variable} ${fontSerif.variable}`}
    >
      <head>
        <ThemeScript />
      </head>
      <body
        className="font-sans bg-[var(--background)] text-[var(--foreground)]"
        suppressHydrationWarning
      >
        <AppShellProvider>
          <SettingsReturnTracker />
          <WorkspaceNavigation />
          <MotionProvider>
            <I18nClientBridge>{children}</I18nClientBridge>
          </MotionProvider>
          <ToastViewport />
        </AppShellProvider>
      </body>
    </html>
  )
}
