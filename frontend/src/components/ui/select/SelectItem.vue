<script setup lang="ts">
import type { SelectItemProps } from "reka-ui"
import type { HTMLAttributes } from "vue"
import { reactiveOmit } from "@vueuse/core"
import { Check } from "@lucide/vue"
import {
  SelectItem,
  SelectItemIndicator,
  SelectItemText,
  useForwardProps,
} from "reka-ui"
import { cn } from "@/lib/utils"

const props = defineProps<SelectItemProps & { class?: HTMLAttributes["class"] }>()

const delegatedProps = reactiveOmit(props, "class")

const forwardedProps = useForwardProps(delegatedProps)
</script>

<template>
  <SelectItem
    data-slot="select-item"
    v-bind="forwardedProps"
    :class="
      cn(
        'focus:bg-accent focus:text-accent-foreground [&_svg:not([class*=\'text-\'])]:text-muted-foreground relative flex w-full cursor-default items-center gap-2 rounded-sm py-1.5 pr-8 pl-2 text-sm outline-hidden select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*=\'size-\'])]:size-4 *:[span]:last:flex *:[span]:last:items-center *:[span]:last:gap-2',
        props.class,
      )
    "
  >
    <span class="absolute right-2 flex size-3.5 items-center justify-center">
      <SelectItemIndicator>
        <slot name="indicator-icon">
          <Check class="size-4" />
        </slot>
      </SelectItemIndicator>
    </span>

    <SelectItemText>
      <slot />
    </SelectItemText>
    <!-- SelectItemText 之外的内容：渲染在下拉项中但不计入选中值的 textContent（即不会回显到触发按钮）。用于"仅下拉显示"的补充信息，如 FFmpeg 版本号。/ Content outside SelectItemText: rendered in the dropdown item but NOT counted in the selected value's textContent (so it never echoes back into the trigger). For dropdown-only extras like the FFmpeg version. -->
    <slot name="extra" />
  </SelectItem>
</template>
