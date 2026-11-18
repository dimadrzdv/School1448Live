import type { Category } from '../api/types'
import type { IconName } from './icons'

// Event tags. The archive groups recordings in this order.
export const CATEGORIES: { id: Category; label: string; plural: string; icon: IconName }[] = [
  { id: 'flag', label: 'Поднятие флага', plural: 'Поднятие флага', icon: 'flag' },
  { id: 'concert', label: 'Концерт', plural: 'Концерты', icon: 'music' },
  { id: 'other', label: 'Другое', plural: 'Другое', icon: 'grid' },
]

export const categoryOf = (id: string) => CATEGORIES.find((c) => c.id === id) ?? CATEGORIES[CATEGORIES.length - 1]
