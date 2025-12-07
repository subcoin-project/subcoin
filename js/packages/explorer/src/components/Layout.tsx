import { ReactNode } from "react";
import { Link, useLocation } from "react-router-dom";

interface LayoutProps {
  children: ReactNode;
}

export function Layout({ children }: LayoutProps) {
  const location = useLocation();

  const isActive = (path: string) => location.pathname === path;

  return (
    <div className="min-h-screen bg-bitcoin-darker flex flex-col">
      {/* Header */}
      <header className="bg-bitcoin-dark border-b border-gray-800">
        <div className="max-w-7xl mx-auto px-4 py-4">
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-8">
              <Link to="/" className="flex items-center space-x-2">
                <span className="text-2xl font-bold text-bitcoin-orange">
                  Subcoin
                </span>
                <span className="text-gray-400">Explorer</span>
              </Link>

              <nav className="flex space-x-4">
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
                  to="/network"
                  className={`px-3 py-2 rounded-md text-sm font-medium ${
                    isActive("/network")
                      ? "bg-bitcoin-orange text-white"
                      : "text-gray-300 hover:bg-gray-800"
                  }`}
                >
                  Network
                </Link>
              </nav>
            </div>

            <div className="text-gray-500 text-sm">
              Node: localhost:9944
            </div>
          </div>
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
