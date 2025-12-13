import { ReactNode, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { SearchBar } from "./SearchBar";
import { EndpointSettings } from "./EndpointSettings";

interface LayoutProps {
  children: ReactNode;
}

export function Layout({ children }: LayoutProps) {
  const location = useLocation();
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);

  const isActive = (path: string) => location.pathname === path;

  return (
    <div className="min-h-screen bg-bitcoin-darker flex flex-col">
      {/* Header */}
      <header className="bg-bitcoin-dark border-b border-gray-800">
        <div className="max-w-7xl mx-auto px-4 py-3 lg:py-4">
          {/* Desktop header */}
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-4 lg:space-x-8">
              <Link to="/" className="flex items-center space-x-2">
                <span className="text-xl lg:text-2xl font-bold text-bitcoin-orange">
                  Subcoin
                </span>
                <span className="text-gray-400 hidden sm:inline">Explorer</span>
              </Link>

              {/* Desktop nav */}
              <nav className="hidden md:flex space-x-2 lg:space-x-4">
                <Link
                  to="/"
                  className={`px-3 py-2 rounded-md text-sm font-medium ${
                    isActive("/")
                      ? "bg-bitcoin-orange text-white"
                      : "text-gray-300 hover:bg-gray-800"
                  }`}
                >
                  Dashboard
                </Link>
                <Link
                  to="/blocks"
                  className={`px-3 py-2 rounded-md text-sm font-medium ${
                    isActive("/blocks")
                      ? "bg-bitcoin-orange text-white"
                      : "text-gray-300 hover:bg-gray-800"
                  }`}
                >
                  Blocks
                </Link>
              </nav>
            </div>

            {/* Desktop right section */}
            <div className="hidden lg:flex items-center space-x-4">
              <SearchBar />
              <EndpointSettings />
            </div>

            {/* Tablet: just endpoint settings */}
            <div className="hidden md:flex lg:hidden items-center space-x-3">
              <EndpointSettings />
            </div>

            {/* Mobile menu button */}
            <button
              onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
              className="md:hidden p-2 text-gray-400 hover:text-gray-200"
              aria-label="Toggle menu"
            >
              {mobileMenuOpen ? (
                <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                </svg>
              ) : (
                <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 6h16M4 12h16M4 18h16" />
                </svg>
              )}
            </button>
          </div>

          {/* Mobile menu */}
          {mobileMenuOpen && (
            <div className="md:hidden mt-4 pt-4 border-t border-gray-800">
              <nav className="flex flex-col space-y-2 mb-4">
                <Link
                  to="/"
                  onClick={() => setMobileMenuOpen(false)}
                  className={`px-3 py-2 rounded-md text-sm font-medium ${
                    isActive("/")
                      ? "bg-bitcoin-orange text-white"
                      : "text-gray-300 hover:bg-gray-800"
                  }`}
                >
                  Dashboard
                </Link>
                <Link
                  to="/blocks"
                  onClick={() => setMobileMenuOpen(false)}
                  className={`px-3 py-2 rounded-md text-sm font-medium ${
                    isActive("/blocks")
                      ? "bg-bitcoin-orange text-white"
                      : "text-gray-300 hover:bg-gray-800"
                  }`}
                >
                  Blocks
                </Link>
              </nav>
              <div className="mb-4">
                <SearchBar />
              </div>
              <div className="flex items-center justify-end text-sm">
                <EndpointSettings />
              </div>
            </div>
          )}
        </div>
      </header>

      {/* Main content */}
      <main className="max-w-7xl mx-auto px-4 py-6 flex-1 w-full">{children}</main>

      {/* Footer */}
      <footer className="bg-bitcoin-dark border-t border-gray-800">
        <div className="max-w-7xl mx-auto px-4 py-4">
          <p className="text-center text-gray-500 text-sm">
            Subcoin Explorer - A Bitcoin full node built on Substrate
          </p>
        </div>
      </footer>
    </div>
  );
}
