import type { Ref } from "vue"

type Updater<T> = T | ((old: T) => T)

export function valueUpdater<T>(updaterOrValue: Updater<T>, ref: Ref<T>) {
  ref.value = typeof updaterOrValue === "function"
    ? (updaterOrValue as (old: T) => T)(ref.value)
    : updaterOrValue
}
