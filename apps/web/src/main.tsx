import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App.js';
import { WebSocketProvider } from './ws/WebSocketContext.js';
import { useUiStore } from './store/uiStore.js';
import './index.css';

// Apply the persisted/default theme before first paint.
document.documentElement.setAttribute('data-theme', useUiStore.getState().theme);

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <WebSocketProvider>
      <App />
    </WebSocketProvider>
  </React.StrictMode>
);
