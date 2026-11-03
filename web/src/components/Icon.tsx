// One icon set (Lucide) at one size and stroke, aligned with the text it sits next to.

import {
  ArrowLeft,
  Check,
  ChevronLeft,
  ChevronRight,
  LayoutGrid,
  LogOut,
  Maximize2,
  MessageSquare,
  Minus,
  Move,
  Pencil,
  Play,
  Plus,
  Search,
  Settings,
  SlidersHorizontal,
  Trash2,
  Users,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";

const ICONS = {
  back: ArrowLeft,
  check: Check,
  prev: ChevronLeft,
  next: ChevronRight,
  layout: LayoutGrid,
  logout: LogOut,
  resize: Maximize2,
  chat: MessageSquare,
  minus: Minus,
  move: Move,
  edit: Pencil,
  play: Play,
  plus: Plus,
  search: Search,
  settings: Settings,
  sliders: SlidersHorizontal,
  trash: Trash2,
  viewers: Users,
  sound: Volume2,
  mute: VolumeX,
  close: X,
};

export type IconName = keyof typeof ICONS;

export function Icon({ name, label }: { name: IconName; label?: string }) {
  const C = ICONS[name];
  return <C className="icon" strokeWidth={2} aria-hidden={label ? undefined : true} aria-label={label} />;
}
