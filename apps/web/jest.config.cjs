module.exports = {
  testEnvironment: 'node',
  testMatch: ['<rootDir>/src/**/*.test.ts'],
  transform: {
    '^.+\\.tsx?$': ['ts-jest', {
      tsconfig: { module: 'CommonJS', isolatedModules: true, composite: false },
    }],
  },
  moduleNameMapper: { '^(\\.{1,2}/.*)\\.js$': '$1' },
  collectCoverageFrom: [
    'src/store/gahStore.ts',
    'src/lib/{format,starMapLayout,skillFrontMatter,icons,workKey,planningTarget,reviewLabels}.ts',
  ],
  coverageReporters: ['text-summary'],
};
