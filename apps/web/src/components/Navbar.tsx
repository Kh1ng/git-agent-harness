import React from 'react';
import type { Page } from '../App.js';

type NavbarProps = {
  currentPage: Page;
  onPageChange: (page: Page) => void;
};

const navItems = [
  { id: 'dashboard' as const, label: 'Dashboard', icon: '🏠' },
  { id: 'sessions' as const, label: 'Sessions', icon: '💻' },
  { id: 'providers' as const, label: 'Providers', icon: '🔧' },
];

export function Navbar({ currentPage, onPageChange }: NavbarProps) {
  return (
    <nav className="bg-white border-b border-gray-200 shadow-sm">
      <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
        <div className="flex justify-between h-16">
          <div className="flex items-center">
            <div className="flex-shrink-0 flex items-center">
              <h1 className="text-xl font-bold text-gray-900">
                Git Agent Harness
              </h1>
            </div>
            
            <div className="ml-10 flex items-baseline space-x-4">
              {navItems.map((item) => (
                <button
                  key={item.id}
                  onClick={() => onPageChange(item.id)}
                  className={`px-3 py-2 rounded-md text-sm font-medium transition-colors ${
                    currentPage === item.id
                      ? 'bg-blue-100 text-blue-700'
                      : 'text-gray-600 hover:text-gray-900 hover:bg-gray-50'
                  }`}
                >
                  <span className="mr-1">{item.icon}</span>
                  {item.label}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>
    </nav>
  );
}