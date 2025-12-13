interface SkeletonProps {
  className?: string;
}

export function Skeleton({ className = "" }: SkeletonProps) {
  return (
    <div
      className={`animate-pulse bg-gray-700 rounded ${className}`}
    />
  );
}

export function SkeletonText({ className = "" }: SkeletonProps) {
  return <Skeleton className={`h-4 ${className}`} />;
}

export function StatCardSkeleton() {
  return (
    <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
      <Skeleton className="h-4 w-24 mb-2" />
      <Skeleton className="h-8 w-32" />
    </div>
  );
}

export function BlockTableRowSkeleton() {
  return (
    <tr className="border-b border-gray-800">
      <td className="px-4 py-3">
        <Skeleton className="h-5 w-20" />
      </td>
      <td className="px-4 py-3">
        <Skeleton className="h-5 w-40" />
      </td>
      <td className="px-4 py-3">
        <Skeleton className="h-5 w-36" />
      </td>
      <td className="px-4 py-3 text-right">
        <Skeleton className="h-5 w-8 ml-auto" />
      </td>
    </tr>
  );
}

export function BlockTableSkeleton({ rows = 10 }: { rows?: number }) {
  return (
    <tbody>
      {Array.from({ length: rows }).map((_, i) => (
        <BlockTableRowSkeleton key={i} />
      ))}
    </tbody>
  );
}

export function TransactionRowSkeleton() {
  return (
    <div className="border-b border-gray-800 py-3">
      <div className="flex items-center justify-between mb-2">
        <Skeleton className="h-5 w-48" />
        <Skeleton className="h-5 w-24" />
      </div>
      <div className="flex gap-4">
        <Skeleton className="h-4 w-32" />
        <Skeleton className="h-4 w-32" />
      </div>
    </div>
  );
}

export function BlockDetailSkeleton() {
  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <Skeleton className="h-8 w-48" />
        <div className="flex gap-2">
          <Skeleton className="h-9 w-24" />
          <Skeleton className="h-9 w-24" />
        </div>
      </div>

      {/* Block info card */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-6">
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {Array.from({ length: 8 }).map((_, i) => (
            <div key={i}>
              <Skeleton className="h-4 w-24 mb-2" />
              <Skeleton className="h-5 w-full max-w-xs" />
            </div>
          ))}
        </div>
      </div>

      {/* Transactions section */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="px-4 py-3 border-b border-gray-800">
          <Skeleton className="h-6 w-32" />
        </div>
        <div className="p-4">
          {Array.from({ length: 5 }).map((_, i) => (
            <TransactionRowSkeleton key={i} />
          ))}
        </div>
      </div>
    </div>
  );
}

export function TransactionDetailSkeleton() {
  return (
    <div className="space-y-6">
      {/* Header */}
      <Skeleton className="h-8 w-64" />

      {/* Tx info card */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-6">
        <div className="space-y-4">
          {Array.from({ length: 4 }).map((_, i) => (
            <div key={i}>
              <Skeleton className="h-4 w-24 mb-2" />
              <Skeleton className="h-5 w-full max-w-md" />
            </div>
          ))}
        </div>
      </div>

      {/* Inputs/Outputs */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
          <div className="px-4 py-3 border-b border-gray-800">
            <Skeleton className="h-6 w-20" />
          </div>
          <div className="p-4 space-y-3">
            {Array.from({ length: 3 }).map((_, i) => (
              <div key={i} className="flex justify-between">
                <Skeleton className="h-5 w-48" />
                <Skeleton className="h-5 w-24" />
              </div>
            ))}
          </div>
        </div>
        <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
          <div className="px-4 py-3 border-b border-gray-800">
            <Skeleton className="h-6 w-20" />
          </div>
          <div className="p-4 space-y-3">
            {Array.from({ length: 3 }).map((_, i) => (
              <div key={i} className="flex justify-between">
                <Skeleton className="h-5 w-48" />
                <Skeleton className="h-5 w-24" />
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

export function NetworkPageSkeleton() {
  return (
    <div className="space-y-6">
      {/* Stats */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        {Array.from({ length: 3 }).map((_, i) => (
          <StatCardSkeleton key={i} />
        ))}
      </div>

      {/* Peers table */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="px-4 py-3 border-b border-gray-800">
          <Skeleton className="h-6 w-32" />
        </div>
        <div className="p-4 space-y-3">
          {Array.from({ length: 5 }).map((_, i) => (
            <div key={i} className="flex justify-between items-center py-2 border-b border-gray-800">
              <Skeleton className="h-5 w-40" />
              <Skeleton className="h-5 w-24" />
              <Skeleton className="h-5 w-20" />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export function AddressDetailSkeleton() {
  return (
    <div className="space-y-6">
      {/* Header */}
      <Skeleton className="h-8 w-48" />

      {/* Address info card */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-6">
        <Skeleton className="h-4 w-24 mb-2" />
        <Skeleton className="h-5 w-full max-w-2xl mb-4" />
        <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
          {Array.from({ length: 4 }).map((_, i) => (
            <div key={i}>
              <Skeleton className="h-4 w-20 mb-2" />
              <Skeleton className="h-6 w-32" />
            </div>
          ))}
        </div>
      </div>

      {/* Transaction history */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="px-4 py-3 border-b border-gray-800">
          <Skeleton className="h-6 w-48" />
        </div>
        <div className="divide-y divide-gray-800">
          {Array.from({ length: 5 }).map((_, i) => (
            <div key={i} className="px-4 py-3">
              <div className="flex justify-between mb-2">
                <Skeleton className="h-5 w-64" />
                <Skeleton className="h-5 w-24" />
              </div>
              <Skeleton className="h-4 w-32" />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
