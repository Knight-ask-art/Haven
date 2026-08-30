import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { RouterProvider } from 'react-router'
import { AppErrorBoundary, router } from './app/router'
import { NoticeProvider } from './app/notice-center/NoticeCenter'
import './index.css'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <NoticeProvider>
      <AppErrorBoundary>
        <RouterProvider router={router} />
      </AppErrorBoundary>
    </NoticeProvider>
  </StrictMode>,
)
