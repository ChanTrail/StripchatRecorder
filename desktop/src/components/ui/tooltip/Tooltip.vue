<!--
  通用 Tooltip 组件（基于 reka-ui）
  在按钮 disabled 时依然可以触发，通过包装层接管鼠标事件。

  General-purpose Tooltip component (built on reka-ui).
  Works even when the trigger button is disabled, via a pointer-events wrapper.

  Props:
  - content: tooltip 文字内容 / tooltip text
  - disabled: 是否禁用 tooltip（不显示）/ whether to suppress the tooltip
  - side: 弹出方向 / popup side (top | bottom | left | right)
  - delayDuration: 延迟显示毫秒数 / delay before showing (ms)
-->
<script setup lang="ts">
import {
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
  TooltipContent,
  TooltipArrow,
} from "reka-ui";

withDefaults(
  defineProps<{
    content?: string;
    disabled?: boolean;
    side?: "top" | "bottom" | "left" | "right";
    delayDuration?: number;
  }>(),
  {
    disabled: false,
    side: "top",
    delayDuration: 400,
  },
);
</script>

<template>
  <TooltipProvider :delay-duration="delayDuration">
    <TooltipRoot :disabled="!content || disabled">
      <!--
        span 包装：pointer-events-auto 确保 disabled 的子按钮依然能触发 hover。
        span wrapper: pointer-events-auto so hover still fires on a disabled child button.
      -->
      <TooltipTrigger as-child>
        <span class="inline-flex pointer-events-auto">
          <slot />
        </span>
      </TooltipTrigger>
      <TooltipContent
        v-if="content"
        :side="side"
        class="z-50 max-w-xs rounded-md bg-popover px-2.5 py-1.5 text-xs text-popover-foreground shadow-md"
      >
        {{ content }}
        <TooltipArrow class="fill-popover" />
      </TooltipContent>
    </TooltipRoot>
  </TooltipProvider>
</template>
