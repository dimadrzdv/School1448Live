import { useEffect, useState } from 'react'
import { Link, NavLink, Outlet, useLocation } from 'react-router'
import { Icon } from '../lib/icons'
import { useLive } from '../lib/live'

export function Layout() {
  const { streams } = useLive()
  const live = (streams?.filter((s) => s.status === 'live' || s.status === 'reconnecting') ?? []).sort(
    (a, b) => Number(b.status === 'live') - Number(a.status === 'live'),
  )
  const { pathname } = useLocation()
  const [scrolled, setScrolled] = useState(false)

  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 4)
    onScroll()
    window.addEventListener('scroll', onScroll, { passive: true })
    return () => window.removeEventListener('scroll', onScroll)
  }, [])

  useEffect(() => {
    window.scrollTo({ top: 0 })
  }, [pathname])

  const liveHref = live.length === 1 ? `/live/${live[0].id}` : '/'

  return (
    <div className="app">
      <header className={`header${scrolled ? ' scrolled' : ''}`}>
        <div className="container">
          <Link to="/" className="brand" aria-label="Школа 1448 — на главную">
            <span className="brand-logo">
              <img src="/branding/school1448.jpg" alt="" width={36} height={32} />
            </span>
            <span className="brand-text">
              <strong>Школа 1448</strong>
              <span>Эфиры и архив мероприятий</span>
            </span>
          </Link>
          <nav className="nav" aria-label="Основные разделы">
            <NavLink to="/" end>
              Главная
            </NavLink>
            <NavLink to="/schedule">Расписание</NavLink>
            <NavLink to="/archive">Архив</NavLink>
          </nav>
          <div className="header-right">
            {live.length > 0 && (
              <Link to={liveHref} className="live-pill">
                <span className="live-dot" />
                В эфире
              </Link>
            )}
          </div>
        </div>
      </header>

      <main>
        <Outlet />
      </main>

      <footer className="footer">
        <div className="container">
          <span>© {new Date().getFullYear()} ГБОУ «Шуваловская школа № 1448»</span>
          <div className="footer-links">
            <a href="https://sch1448.mskobr.ru/" target="_blank" rel="noreferrer">
              Официальный сайт школы
            </a>
            <Link to="/admin">Для администрации</Link>
          </div>
        </div>
      </footer>

      <nav className="tabbar" aria-label="Навигация">
        <NavLink to="/" end>
          <Icon name="home" />
          Главная
        </NavLink>
        {live.length > 0 && (
          <Link to={liveHref} className="is-live">
            <Icon name="radio" />
            Эфир
          </Link>
        )}
        <NavLink to="/schedule">
          <Icon name="calendar" />
          Расписание
        </NavLink>
        <NavLink to="/archive">
          <Icon name="archive" />
          Архив
        </NavLink>
      </nav>
    </div>
  )
}
