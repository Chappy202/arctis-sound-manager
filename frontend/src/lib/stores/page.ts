import { writable } from 'svelte/store';

export type Page = 'mixer' | 'eq' | 'device' | 'spatial' | 'mic';

export const currentPage = writable<Page>('mixer');
