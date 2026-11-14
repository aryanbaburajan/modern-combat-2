export function openSaveDatabase() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open('mc2-saves', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('files');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error('Cannot open saved games'));
    request.onblocked = () => reject(new Error('Saved games are open in another tab'));
  });
}
