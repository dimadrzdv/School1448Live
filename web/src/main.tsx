import { StrictMode, Suspense, lazy } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Link, Route, Routes } from 'react-router'
import { StateCard } from './components/Cards'
import { Layout } from './components/Layout'
import { Home } from './pages/Home'
import './styles/tokens.css'
import './styles/app.css'

// Viewer pages load eagerly (fast first paint); admin tools and the archive player
// are separate chunks so viewers never download them.
const Watch = lazy(() => import('./pages/Watch').then((m) => ({ default: m.Watch })))
const Archive = lazy(() => import('./pages/Archive').then((m) => ({ default: m.Archive })))
const RecordingPage = lazy(() => import('./pages/RecordingPage').then((m) => ({ default: m.RecordingPage })))
const Schedule = lazy(() => import('./pages/Schedule').then((m) => ({ default: m.Schedule })))
const Admin = lazy(() => import('./pages/Admin').then((m) => ({ default: m.Admin })))
const Studio = lazy(() => import('./pages/Studio').then((m) => ({ default: m.Studio })))

function NotFound() {
  return (
    <div className="container section">
      <StateCard icon="alert" title="Страница не найдена">
        <Link to="/" className="btn btn-primary">
          На главную
        </Link>
      </StateCard>
    </div>
  )
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <Suspense fallback={null}>
        <Routes>
          <Route path="/studio/:id" element={<Studio />} />
          <Route element={<Layout />}>
            <Route index element={<Home />} />
            <Route path="live/:id" element={<Watch />} />
            <Route path="schedule" element={<Schedule />} />
            <Route path="archive" element={<Archive />} />
            <Route path="archive/:id" element={<RecordingPage />} />
            <Route path="admin" element={<Admin />} />
            <Route path="*" element={<NotFound />} />
          </Route>
        </Routes>
      </Suspense>
    </BrowserRouter>
  </StrictMode>,
)
