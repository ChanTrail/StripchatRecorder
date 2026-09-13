<script setup lang="ts">
import type { HTMLAttributes } from "vue"
import { NumberFieldInput, injectNumberFieldRootContext } from "reka-ui"
import { cn } from "@/lib/utils"
import { onBeforeUnmount } from "vue"

const props = defineProps<{
  class?: HTMLAttributes["class"]
}>()

const ctx = injectNumberFieldRootContext()

/**
 * reka-ui 内部已有 wheel handler，但它通过 Vue 的 onWheel 注册，Vue 默认注册为
 * passive listener，导致其中的 preventDefault() 被浏览器静默忽略，页面仍会滚动。
 *
 * 解决方案：在真实 DOM 上以 { passive: false, capture: true } 注册我们自己的
 * listener，capture 确保比 reka-ui 的 bubble 阶段 listener 先执行：
 * - preventDefault() 阻止页面滚动（非 passive 才有效）
 * - stopPropagation() 阻止事件继续传播到 reka-ui 的 listener
 * - 手动复现 reka-ui 的值变更逻辑
 *
 * reka-ui registers its wheel handler via Vue's onWheel, which Vue registers as a
 * passive listener — so its internal preventDefault() is silently ignored by the
 * browser and the page still scrolls.
 *
 * Fix: register our own listener on the real DOM with { passive: false, capture: true }.
 * capture: true ensures we run before reka-ui's bubble-phase listener.
 * - preventDefault() blocks page scroll (only works on non-passive listeners)
 * - stopPropagation() prevents the event reaching reka-ui's listener
 * - we manually replicate reka-ui's value-change logic
 */
function handleWheel(e: WheelEvent) {
  if (document.activeElement !== e.currentTarget) return
  if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return
  e.preventDefault()
  e.stopPropagation()
  if (e.deltaY > 0) {
    ctx.invertWheelChange?.value ? ctx.handleIncrease() : ctx.handleDecrease()
  } else {
    ctx.invertWheelChange?.value ? ctx.handleDecrease() : ctx.handleIncrease()
  }
}

let inputEl: HTMLElement | null = null

function onRef(vnode: any) {
  const el: HTMLElement | null = vnode?.$el ?? null
  if (el === inputEl) return
  inputEl?.removeEventListener("wheel", handleWheel, { capture: true })
  inputEl = el
  el?.addEventListener("wheel", handleWheel, { passive: false, capture: true })
}

onBeforeUnmount(() => {
  inputEl?.removeEventListener("wheel", handleWheel, { capture: true })
})
</script>

<template>
  <NumberFieldInput
    data-slot="input"
    :ref="onRef"
    :class="cn('flex h-9 w-full rounded-md border border-input bg-transparent py-1 text-sm text-center shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50', props.class)"
  />
</template>
