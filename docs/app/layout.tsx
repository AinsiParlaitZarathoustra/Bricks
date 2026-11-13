import { RootProvider } from 'fumadocs-ui/provider/next';
import './global.css';
import { Inter } from 'next/font/google';
import type { Metadata } from 'next';

const inter = Inter({
  subsets: ['latin'],
});

export const metadata: Metadata = {
  title: {
    default: 'Bricks',
    template: '%s | Bricks',
  },
  description: 'A desktop coding agent and modular Rust SDK. Tool execution, LLM streaming, memory, sub-agent orchestration, and MCP.',
  openGraph: {
    type: 'website',
    siteName: 'Bricks',
  },
  other: {
    'github:repo': 'https://github.com/AshlarAI/Bricks',
  },
};

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <html lang="en" className={`${inter.className} dark`} style={{ colorScheme: 'dark' }} suppressHydrationWarning>
      <body className="flex flex-col min-h-screen">
        <RootProvider theme={{ enabled: false, defaultTheme: 'dark' }}>
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
