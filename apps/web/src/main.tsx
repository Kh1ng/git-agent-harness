import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App.js';
import { WebSocketProvider } from './ws/WebSocketContext.js';
import './index.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <WebSocketProvider>
      <App />
    </WebSocketProvider>
  </React.StrictMode>
);